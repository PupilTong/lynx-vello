//! The two monitors over the engine threads' jobs, as Lynx's
//! `LongTaskMonitor` and `JSBlockingMonitor` have them: their
//! configurations, their entries (made whole where they are produced and
//! handed to [`PipelineRegistry::deliver_entry`]), and the tasks a monitor
//! runs on the view's `bobcat-main`. Both are off unless the embedder turns
//! them on; off, each costs one relaxed atomic load where it would measure
//! and reads no clock.

use std::future::Future;
use std::pin::pin;
use std::time::Duration;

use tokio::sync::watch;

use super::{PipelineRegistry, Timestamp};
use crate::jobs::{JobKind, JsThreadName};
use crate::link::HostOutbox;

/// The long task monitor's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongTaskConfig {
    /// The shortest job reported as a long task.
    pub threshold: Duration,
}

impl Default for LongTaskConfig {
    /// Lynx's `LongTaskMonitor::duration_threshold_ms_`: 50 ms, which is
    /// also W3C Long Tasks' threshold.
    fn default() -> Self {
        Self {
            threshold: Duration::from_millis(50),
        }
    }
}

/// The JS blocking monitor's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsBlockingConfig {
    /// The shortest wait for the worker thread that is counted.
    pub threshold: Duration,
    /// The `timer` reports' unit: they come 1, 3, 6, 10 … of it after the
    /// `loadBundle` report.
    pub report_interval: Duration,
}

impl Default for JsBlockingConfig {
    /// Lynx's defaults (`lynx_env.cc`): 20 ms and 5000 ms.
    fn default() -> Self {
        Self {
            threshold: Duration::from_millis(20),
            report_interval: Duration::from_secs(5),
        }
    }
}

/// One job of an engine thread that ran at least the long task threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongTaskEntry {
    /// When the job began to run.
    pub start: Timestamp,
    /// When it returned, its synchronous waits included.
    pub end: Timestamp,
    /// The thread it ran on.
    pub thread: JsThreadName,
    /// What it was queued to do.
    pub kind: JobKind,
}

impl LongTaskEntry {
    /// How long the job ran.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.end.duration_since(self.start)
    }
}

/// Which of Lynx's two JS blocking reports a [`JsBlockingEntry`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsBlockingStage {
    /// Made when the view's `pipeline.loadBundle` record completes.
    LoadBundle,
    /// Made by the report timer.
    Timer,
}

impl JsBlockingStage {
    /// Lynx's `stage` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoadBundle => "loadBundle",
            Self::Timer => "timer",
        }
    }
}

/// Lynx's `js_blocking` entry: the waits of the messages sent to the view's
/// workers that were at least the threshold, since the last report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JsBlockingEntry {
    pub stage: JsBlockingStage,
    /// The waits counted since the last report, summed.
    pub total_blocking_time: Duration,
    /// How many waits were counted.
    pub total_blocking_count: u32,
    /// From the last report made, or the monitor's start, to this one.
    pub total_duration: Duration,
    /// `total_blocking_time / total_duration`; overlapping waits can take
    /// it past 1.
    pub blocking_ratio: f64,
    /// `total_blocking_time / total_blocking_count`.
    pub avg_blocking_time: Duration,
    /// A `timer` report's time since the `loadBundle` report.
    pub time_after_load_bundle: Option<Duration>,
}

impl JsBlockingEntry {
    /// The report Lynx's `ReportBlockingInfo` makes of these totals.
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
            // Lynx divides by a zero duration as well; here that is an
            // infinite ratio, which the JSON writer spells `null`.
            blocking_ratio: total_blocking_time.as_secs_f64() / total_duration.as_secs_f64(),
            avg_blocking_time: total_blocking_time
                .checked_div(total_blocking_count)
                .unwrap_or_default(),
            time_after_load_bundle,
        }
    }
}

/// A threshold as the registry's atomics hold it: microseconds plus one,
/// so that 0 is "off" and a zero threshold is still a monitor that is on.
pub(crate) fn encode_threshold(threshold: Option<Duration>) -> u64 {
    threshold.map_or(0, |threshold| {
        u64::try_from(threshold.as_micros())
            .unwrap_or(u64::MAX)
            .min(u64::MAX - 1)
            + 1
    })
}

/// The threshold [`encode_threshold`] wrote; `None` is a monitor that is
/// off.
pub(crate) const fn decode_threshold(raw: u64) -> Option<Duration> {
    if raw == 0 {
        None
    } else {
        Some(Duration::from_micros(raw - 1))
    }
}

/// A task a monitor runs on the view's `bobcat-main`
/// ([`crate::link::ToMain::StartMonitorTask`]), until the view ends or the
/// monitor's configuration changes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum MonitorTask {
    /// The JS blocking monitor's `timer` reports.
    JsBlockingReports { epoch: u64, interval: Duration },
}

impl MonitorTask {
    pub(crate) async fn serve(self, host: HostOutbox) {
        match self {
            Self::JsBlockingReports { epoch, interval } => {
                serve_js_blocking_reports(host, epoch, interval).await;
            }
        }
    }
}

/// What a monitor task waits with: the view's end, and the registry's
/// monitor epochs ([`PipelineRegistry::monitor_epochs`]), at each of whose
/// notifications `current` says whether the configuration of `epoch` is
/// still its monitor's.
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

    /// Whether the configuration of this task's epoch is still current.
    pub(crate) fn is_current(&self) -> bool {
        (self.current)(self.host.registry(), self.epoch)
    }

    /// Waits for `until` and answers what it gave, or `None` once the view
    /// ended or the configuration was replaced first.
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

    /// Waits for the next notification of the monitor epochs; `false` is
    /// the view's end.
    pub(crate) async fn changed(&mut self) -> bool {
        tokio::select! {
            biased;
            () = self.host.token().cancelled() => false,
            changed = self.epochs.changed() => changed.is_ok(),
        }
    }
}

/// The JS blocking monitor's `timer` reports: from the `loadBundle` report
/// (Lynx's `OnPerformanceEvent`), on Lynx's `ReportWithTimer` schedule, each
/// one interval more after the previous than the gap before it — 1, 3, 6,
/// 10 … intervals after the load.
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
