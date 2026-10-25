//! The two monitors over the engine threads' jobs, as Lynx's
//! `LongTaskMonitor` and `JSBlockingMonitor` have them: their switches,
//! what each records, and the entries [`crate::LynxView::pump`] assembles
//! from those records.
//!
//! Both are off unless the embedder turns them on
//! ([`crate::LynxView::set_long_task_monitor`],
//! [`crate::LynxView::set_js_blocking_monitor`]), as Lynx's
//! `enable_long_task_timing` and `enable_js_blocking_monitor` are. Off, each
//! costs one relaxed atomic load where it would measure — per job for the
//! long task monitor, per message sent to a worker for the JS blocking
//! monitor — and reads no clock.

use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use super::Timestamp;
use crate::clock::ClockInstant;
use crate::jobs::{JobKind, JsThreadName};
use crate::link::HostOutbox;

/// The long task monitor's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongTaskConfig {
    /// The shortest job reported: a job that ran at least this long is a
    /// long task.
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
    /// The shortest wait counted: a message whose job waited at least this
    /// long for the worker thread adds to the blocking totals.
    pub threshold: Duration,
    /// The interval the `timer` reports are spaced by, Lynx's way: the
    /// first one interval after the `loadBundle` report, and each after
    /// that one interval more than the gap before it.
    pub report_interval: Duration,
}

impl Default for JsBlockingConfig {
    /// Lynx's `js_blocking_threshold_ms` (20) and
    /// `js_blocking_report_interval_ms` (5000) defaults
    /// (`lynx_env.cc`).
    fn default() -> Self {
        Self {
            threshold: Duration::from_millis(20),
            report_interval: Duration::from_secs(5),
        }
    }
}

/// One job of an engine thread that ran at least the long task monitor's
/// threshold: W3C Long Tasks' `longtask`, attributed the way Lynx's
/// `lynxsdk_long_task_timing` event attributes one.
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
    /// Made by the report timer that starts with the `loadBundle` report.
    Timer,
}

impl JsBlockingStage {
    /// Lynx's `stage` value: `"loadBundle"` or `"timer"`
    /// (`kJSBlockingStageLoadBundle`, `kJSBlockingStageTimer`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoadBundle => "loadBundle",
            Self::Timer => "timer",
        }
    }
}

/// Lynx's `js_blocking` entry: how long the messages sent to the view's
/// workers waited for the worker thread, summed over those that waited at
/// least the monitor's threshold, since the last report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JsBlockingEntry {
    pub stage: JsBlockingStage,
    /// The waits counted since the last report, summed.
    pub total_blocking_time: Duration,
    /// How many waits were counted.
    pub total_blocking_count: u32,
    /// From the last report made — or, for the first, from the monitor's
    /// start — to this one.
    pub total_duration: Duration,
    /// `total_blocking_time / total_duration`. Several waits can overlap,
    /// so it may exceed 1.
    pub blocking_ratio: f64,
    /// `total_blocking_time / total_blocking_count`.
    pub avg_blocking_time: Duration,
    /// For a [`JsBlockingStage::Timer`] report, how long after the
    /// `loadBundle` report it was made.
    pub time_after_load_bundle: Option<Duration>,
}

/// One long task, as a job recorded it: plain instants, made into a
/// [`LongTaskEntry`] by whoever takes it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LongTaskRecord {
    pub(crate) start: ClockInstant,
    pub(crate) end: ClockInstant,
    pub(crate) thread: JsThreadName,
    pub(crate) kind: JobKind,
}

impl LongTaskRecord {
    pub(crate) const fn entry(&self) -> LongTaskEntry {
        LongTaskEntry {
            start: Timestamp(self.start),
            end: Timestamp(self.end),
            thread: self.thread,
            kind: self.kind,
        }
    }
}

/// One JS blocking report as the registry made it: the totals and spans
/// Lynx's `ReportBlockingInfo` reads, made into a [`JsBlockingEntry`] — the
/// ratio and the average — by whoever takes it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct JsBlockingRecord {
    pub(crate) stage: JsBlockingStage,
    pub(crate) total_time: Duration,
    pub(crate) total_count: u32,
    pub(crate) total_duration: Duration,
    pub(crate) time_after_load_bundle: Option<Duration>,
}

impl JsBlockingRecord {
    pub(crate) fn entry(&self) -> JsBlockingEntry {
        let duration = self.total_duration.as_secs_f64();
        JsBlockingEntry {
            stage: self.stage,
            total_blocking_time: self.total_time,
            total_blocking_count: self.total_count,
            total_duration: self.total_duration,
            // Lynx divides by a zero duration as well; here that is an
            // infinite ratio, which the JSON writer spells `null`.
            blocking_ratio: self.total_time.as_secs_f64() / duration,
            avg_blocking_time: self
                .total_time
                .checked_div(self.total_count)
                .unwrap_or_default(),
            time_after_load_bundle: self.time_after_load_bundle,
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

/// What the JS blocking monitor's report task watches: which configuration
/// is current, and the instant its timer reports count from once there is
/// one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct JsBlockingSignal {
    /// Bumped by every [`crate::LynxView::set_js_blocking_monitor`]: a task
    /// started for an earlier one ends.
    pub(crate) epoch: u64,
    /// The `loadBundle` report's instant, or the enabling's for a monitor
    /// turned on after the view's load completed; `None` while neither has
    /// happened.
    pub(crate) anchor: Option<ClockInstant>,
}

/// The JS blocking monitor's `timer` reports, as a task of the view on
/// `bobcat-main`: started when the monitor is turned on, and ended by the
/// view's end or by the next change of the monitor's configuration.
///
/// It waits for the `loadBundle` report, as Lynx's `JSBlockingMonitor`
/// starts its timer there (`OnPerformanceEvent`), and then reports on
/// Lynx's schedule (`ReportWithTimer`): the first report one interval after
/// that, and each report after the previous one by one interval more than
/// the gap before it — 1, 3, 6, 10, … intervals after the load. Each report
/// wakes the embedder, whose next [`crate::LynxView::pump`] takes it; a
/// report with nothing counted is not made, as in Lynx.
pub(crate) async fn serve_js_blocking_reports(host: HostOutbox, epoch: u64, interval: Duration) {
    let registry = Arc::clone(host.registry());
    let mut signal = registry.js_blocking_signal();
    let anchor = loop {
        let current = *signal.borrow_and_update();
        if current.epoch != epoch {
            return;
        }
        if let Some(anchor) = current.anchor {
            break anchor;
        }
        tokio::select! {
            biased;
            () = host.token().cancelled() => return,
            changed = signal.changed() => if changed.is_err() { return },
        }
    };
    let mut deadline = anchor;
    let mut index: u32 = 1;
    loop {
        let Some(next) = deadline.checked_add(interval.saturating_mul(index)) else {
            return;
        };
        deadline = next;
        let mut sleep = pin!(crate::clock::sleep_until(deadline));
        loop {
            tokio::select! {
                biased;
                () = host.token().cancelled() => return,
                changed = signal.changed() => {
                    if changed.is_err() || signal.borrow_and_update().epoch != epoch {
                        return;
                    }
                }
                () = &mut sleep => break,
            }
        }
        if registry.report_js_blocking(epoch) {
            host.wake();
        }
        index = index.saturating_add(1);
    }
}
