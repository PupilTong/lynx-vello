//! The entry types, their Lynx field table and the registry's record life,
//! without a view: the registry is driven by hand, as a flush, a painter
//! and the two consumers would drive it.

use std::sync::Arc;
use std::time::Duration;

use terminus::link::painter::PresentedFrame;

use super::*;
use crate::background::WorkerKey;

fn at(origin: &TimeOrigin, millis: u64) -> Timestamp {
    Timestamp(origin.instant + Duration::from_millis(millis))
}

fn pipeline(origin: &TimeOrigin) -> PipelineTiming {
    PipelineTiming {
        origin: PipelineOrigin::UpdateTriggeredByBts,
        identifier: Some(Arc::from("flag \"one\"")),
        pipeline_start: at(origin, 1),
        pipeline_end: at(origin, 9),
        mts_render: Some(at(origin, 2)..at(origin, 3)),
        resolve: at(origin, 3)..at(origin, 4),
        layout: at(origin, 4)..at(origin, 5),
        painting_ui_operation_execute: at(origin, 5)..at(origin, 6),
        layout_ui_operation_execute: at(origin, 7)..at(origin, 8),
        paint_end: at(origin, 9),
        framework_rendering_timing: FrameworkTiming {
            dsl: Some(Arc::from("reactLynx")),
            stage: Some(Arc::from("update")),
            marks: vec![(Arc::from("diffVdomStart"), at(origin, 1))],
        },
        host_platform_timing: PainterTiming {
            compose: at(origin, 7)..at(origin, 8),
        },
        actual_fmp: None,
    }
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "the origin's own reading converts exactly, both rounded alike"
)]
fn the_origin_instant_converts_to_the_origin_unix_reading() {
    let origin = TimeOrigin::now();
    assert_eq!(origin.instant().unix_millis(&origin), origin.unix_millis());
    let later = Timestamp(origin.instant + Duration::from_micros(1_500));
    assert!((later.unix_millis(&origin) - origin.unix_millis() - 1.5).abs() < 1e-6);
    let wall = SystemTime::now();
    let converted = origin
        .timestamp_of(wall)
        .expect("a reading of now converts");
    assert!((converted.unix_millis(&origin) - unix_millis_of(wall)).abs() < 0.01);
}

#[test]
fn origins_round_trip_through_their_lynx_names() {
    for origin in PipelineOrigin::ALL {
        assert_eq!(
            PipelineOrigin::from_lynx_name(origin.lynx_name()),
            Some(origin)
        );
    }
    assert_eq!(PipelineOrigin::from_lynx_name("nope"), None);
}

#[test]
fn a_pipeline_entry_writes_lynx_json_with_its_nested_objects() {
    let origin = TimeOrigin::now();
    let entry = PerformanceEntry::Pipeline(Box::new(pipeline(&origin)));
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["entryType"], "pipeline");
    assert_eq!(value["name"], "updateTriggeredByBts");
    assert_eq!(value["identifier"], "flag \"one\"");
    assert_eq!(value["frameworkRenderingTiming"]["dsl"], "reactLynx");
    assert_eq!(value["frameworkRenderingTiming"]["stage"], "update");
    assert!(value["frameworkRenderingTiming"]["diffVdomStart"].is_number());
    assert_eq!(value["hostPlatformTiming"]["hostPlatformType"], "vello");
    let paint_end = value["paintEnd"].as_f64().expect("a number");
    assert!((paint_end - origin.unix_millis() - 9.0).abs() < 1e-6);
    let compose = value["hostPlatformTiming"]["composeEnd"].as_f64().unwrap();
    assert!((compose - origin.unix_millis() - 8.0).abs() < 1e-6);
    // Absent fields are left out, not zeroed.
    assert!(value.get("actualFmp").is_none());
    assert!(value.get("loadBundleStart").is_none());
}

#[test]
fn the_record_encoding_names_each_entry_s_pair_count() {
    let origin = TimeOrigin::now();
    let entry = PerformanceEntry::Pipeline(Box::new(pipeline(&origin)));
    let pairs = entry.lynx_fields(&origin).count();
    let record = encode_entries(&[entry], &origin);
    assert!(record.starts_with(&format!("{}:{pairs}", pairs.to_string().len())));
    assert!(record.contains("9:entryType9:spipeline"));
}

fn frame(commit_id: u64) -> PresentedFrame {
    let now = ClockInstant::now();
    PresentedFrame {
        commit_id,
        compose_start: now,
        compose_end: now,
        presented: now,
    }
}

fn flush(opener: OpenerId, pipeline: u64, flags: &[&str], commit_id: u64) -> FlushBinding {
    FlushBinding {
        opener,
        pipeline,
        flags: flags.iter().map(|flag| Arc::from(*flag)).collect(),
        native: Vec::new(),
        load: false,
        commit_id,
        timing: None,
        started: ClockInstant::now(),
    }
}

#[test]
fn a_flagged_flush_completes_at_the_present_of_its_commit() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::PIPELINE);
    let id = registry.generate_id();
    registry.open(
        id,
        Some(PipelineOrigin::UpdateTriggeredByBts),
        "reactLynx",
        "update",
        true,
        Opener::Realm(OpenerId::PAGE),
    );
    registry.mark(id, "diffVdomStart");
    assert!(!registry.bind_commit(flush(OpenerId::PAGE, id, &["f"], 3)));
    // An older commit's present completes nothing.
    assert!(registry.note_presented(frame(2)).is_empty());
    assert_eq!(registry.generation(), 0);
    registry.note_presented(frame(3));
    assert_eq!(registry.generation(), 1);
    let entries = registry.take_completed_for_rust();
    let [PerformanceEntry::Pipeline(entry)] = entries.as_slice() else {
        panic!("one pipeline entry: {entries:?}");
    };
    assert_eq!(entry.identifier.as_deref(), Some("f"));
    assert_eq!(entry.origin, PipelineOrigin::UpdateTriggeredByBts);
    assert_eq!(entry.framework_rendering_timing.marks.len(), 1);
    assert!(registry.records_for_test().is_empty(), "taken and freed");
    // A flag fires once.
    let again = registry.generate_id();
    registry.open(again, None, "", "", true, Opener::Realm(OpenerId::PAGE));
    registry.bind_commit(flush(OpenerId::PAGE, again, &["f"], 4));
    assert!(registry.records_for_test().is_empty());
}

#[test]
fn a_flush_bound_to_a_presented_frame_completes_at_once() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::ALL);
    registry.note_presented(frame(5));
    assert!(registry.bind_commit(flush(OpenerId::PAGE, 0, &["late"], 5)));
    assert_eq!(registry.take_completed_for_rust().len(), 1);
}

#[test]
fn a_record_no_audience_wants_is_freed_at_completion() {
    let registry = PipelineRegistry::detached();
    registry.bind_commit(flush(OpenerId::PAGE, 0, &["nobody"], 1));
    assert_eq!(registry.records_for_test().len(), 1);
    registry.note_presented(frame(1));
    assert!(registry.records_for_test().is_empty());
    assert_eq!(registry.generation(), 0);
}

#[test]
fn an_unflagged_pipeline_is_not_recorded_while_nothing_observes() {
    let registry = PipelineRegistry::detached();
    let id = registry.generate_id();
    registry.open(id, None, "", "", false, Opener::Realm(OpenerId::PAGE));
    assert!(registry.records_for_test().is_empty());
    assert!(!registry.claim_for_flush(id, false));
}

#[test]
fn a_worker_observer_is_owed_the_entries_of_its_realm() {
    let registry = PipelineRegistry::detached();
    let worker = WorkerKey::new(7);
    registry.bind_worker(worker, OpenerId::PAGE);
    registry.observe_worker(worker, true, false);
    registry.bind_commit(flush(OpenerId::PAGE, 0, &["mine"], 1));
    assert_eq!(registry.note_presented(frame(1)), vec![worker]);
    assert!(registry.take_completed_for_rust().is_empty());
    let entries = registry.take_completed_for_worker(worker);
    assert_eq!(entries.len(), 1);
    assert!(registry.take_completed_for_worker(worker).is_empty());
    assert!(registry.records_for_test().is_empty());
}

#[test]
fn a_load_waits_for_its_background_entry_and_reports_extra_timing_again() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::LOAD_BUNDLE);
    let worker = WorkerKey::new(1);
    let _ = registry.mark_worker_load(worker, WorkerLoadMark::CoreStart);
    registry.open_load(OpenerId::PAGE, "app:///main.js", None);
    registry.bind_worker(worker, OpenerId::PAGE);
    registry.mark_load(OpenerId::PAGE, LoadMark::VmExecuteEnd);
    assert!(registry.mark_load(OpenerId::PAGE, LoadMark::MtsRenderStart));
    let mut binding = flush(OpenerId::PAGE, 0, &[], 1);
    binding.load = true;
    registry.bind_commit(binding);
    registry.note_presented(frame(1));
    assert_eq!(registry.generation(), 0, "the BTS entry has not loaded");
    for mark in [WorkerLoadMark::CoreEnd, WorkerLoadMark::BackgroundStart] {
        let _ = registry.mark_worker_load(worker, mark);
    }
    let completion = registry.mark_worker_load(worker, WorkerLoadMark::BackgroundEnd);
    assert!(completion.completed);
    assert!(!completion.owed_to_caller, "the worker does not observe");
    let entries = registry.take_completed_for_rust();
    let [PerformanceEntry::LoadBundle(load)] = entries.as_slice() else {
        panic!("one load entry: {entries:?}");
    };
    assert!(load.load_core.is_some() && load.load_background.is_some());
    assert!(load.fcp.is_none() && load.total_fcp.is_none());
    let extra = ExtraTiming {
        open_time: Some(SystemTime::now()),
        prepare_template_start: Some(SystemTime::now()),
        ..ExtraTiming::default()
    };
    assert!(registry.set_extra_timing(&extra));
    let entries = registry.take_completed_for_rust();
    let [PerformanceEntry::LoadBundle(load)] = entries.as_slice() else {
        panic!("the load again: {entries:?}");
    };
    assert!(load.fcp.is_some() && load.total_fcp.is_some());
    assert!(!registry.set_extra_timing(&extra), "a key is set once");
}

#[test]
fn a_paint_entry_writes_its_three_fields() {
    let origin = TimeOrigin::now();
    let entry = PerformanceEntry::Paint(PaintEntry {
        name: PaintName::FirstContentfulPaint,
        start: at(&origin, 4),
    });
    assert_eq!(entry.entry_type(), "paint");
    assert_eq!(entry.name(), "first-contentful-paint");
    assert!(entry.pipeline().is_none());
    let keys: Vec<_> = entry
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    assert_eq!(keys, ["entryType", "name", "startTime"]);
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["entryType"], "paint");
    assert_eq!(value["name"], "first-contentful-paint");
    let start = value["startTime"].as_f64().expect("a number");
    assert!((start - origin.unix_millis() - 4.0).abs() < 1e-6);
    assert_eq!(PaintName::FirstPaint.as_str(), "first-paint");
    let record = encode_entries(&[entry], &origin);
    assert!(record.starts_with("1:3"), "{record}");
    assert!(record.contains("9:entryType6:spaint"), "{record}");
}

#[test]
fn paint_marks_complete_once_for_whoever_asked_for_paint() {
    let registry = PipelineRegistry::detached();
    let worker = WorkerKey::new(3);
    let other = WorkerKey::new(4);
    registry.observe_rust(EntryTypes::PAINT);
    registry.observe_worker(worker, true, true);
    registry.observe_worker(other, true, false);
    assert!(registry.wants_content_detection());
    let first = frame(1);
    assert_eq!(registry.note_paint(Some(first), None), vec![worker]);
    assert!(
        registry.wants_content_detection(),
        "no contentful paint yet"
    );
    let contentful = frame(2);
    assert_eq!(
        registry.note_paint(Some(first), Some(contentful)),
        vec![worker]
    );
    assert!(!registry.wants_content_detection());
    assert!(
        registry
            .note_paint(Some(first), Some(contentful))
            .is_empty()
    );
    let names = |entries: Vec<PerformanceEntry>| -> Vec<(PaintName, Timestamp)> {
        entries
            .into_iter()
            .map(|entry| match entry {
                PerformanceEntry::Paint(paint) => (paint.name, paint.start),
                entry => panic!("a paint entry: {entry:?}"),
            })
            .collect()
    };
    let expected = vec![
        (PaintName::FirstPaint, Timestamp(first.presented)),
        (
            PaintName::FirstContentfulPaint,
            Timestamp(contentful.presented),
        ),
    ];
    assert_eq!(names(registry.take_completed_for_rust()), expected);
    assert_eq!(names(registry.take_completed_for_worker(worker)), expected);
    assert!(registry.take_completed_for_worker(other).is_empty());
    assert!(registry.records_for_test().is_empty(), "taken and freed");
}

#[test]
fn paint_marks_nobody_asked_for_make_no_record_and_no_detection() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::LOAD_BUNDLE | EntryTypes::PIPELINE);
    assert!(!registry.wants_content_detection());
    assert!(
        registry.wants_presentation_timing(),
        "the first paint is not recorded yet"
    );
    registry.note_paint(Some(frame(1)), None);
    assert!(!registry.wants_presentation_timing());
    assert_eq!(registry.generation(), 0);
    assert!(registry.records_for_test().is_empty());
    // A worker that observes pipelines only asks for no detection either.
    registry.observe_worker(WorkerKey::new(1), true, false);
    assert!(!registry.wants_content_detection());
    // Asking for paint turns detection on until the contentful paint.
    registry.observe_worker(WorkerKey::new(1), true, true);
    assert!(registry.wants_content_detection());
    assert!(registry.wants_presentation_timing());
    registry.forget_worker(WorkerKey::new(1));
    assert!(!registry.wants_content_detection());
    assert!(!registry.wants_presentation_timing());
}

#[test]
fn a_waiting_record_wants_presentation_timing_and_an_untimed_present_completes_it() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::PIPELINE);
    registry.note_paint(Some(frame(1)), None);
    assert!(!registry.wants_presentation_timing());
    registry.bind_commit(flush(OpenerId::PAGE, 0, &["waits"], 2));
    assert!(registry.wants_presentation_timing());
    // The painter drew commit 2 before it read the switch on.
    assert!(registry.note_untimed_present(2).is_empty());
    assert_eq!(registry.generation(), 1);
    assert!(!registry.wants_presentation_timing());
    assert_eq!(registry.take_completed_for_rust().len(), 1);
}

fn long_task(start: ClockInstant, millis: u64, kind: JobKind) -> LongTaskRecord {
    LongTaskRecord {
        start,
        end: start + Duration::from_millis(millis),
        thread: JsThreadName::Workers,
        kind,
    }
}

#[test]
fn the_long_task_switch_is_off_by_default_and_keeps_a_zero_threshold() {
    let registry = PipelineRegistry::detached();
    assert_eq!(registry.long_task_threshold(), None);
    registry.set_long_task_threshold(Some(Duration::ZERO));
    assert_eq!(registry.long_task_threshold(), Some(Duration::ZERO));
    registry.set_long_task_threshold(Some(Duration::from_millis(50)));
    assert_eq!(
        registry.long_task_threshold(),
        Some(Duration::from_millis(50))
    );
    registry.set_long_task_threshold(None);
    assert_eq!(registry.long_task_threshold(), None);
    assert_eq!(
        LongTaskConfig::default().threshold,
        Duration::from_millis(50)
    );
}

#[test]
fn a_long_task_is_kept_only_for_an_embedder_observing_long_tasks() {
    let registry = PipelineRegistry::detached();
    let start = ClockInstant::now();
    // Nobody observes: dropped as it is made, and nothing to wake for.
    assert!(!registry.note_long_task(long_task(start, 60, JobKind::Post)));
    assert_eq!(registry.generation(), 0);
    registry.observe_rust(EntryTypes::PIPELINE);
    assert!(!registry.note_long_task(long_task(start, 60, JobKind::Post)));
    registry.observe_rust(EntryTypes::LONG_TASK);
    assert!(registry.note_long_task(long_task(start, 60, JobKind::Timer)));
    assert_eq!(registry.generation(), 1);
    let entries = registry.take_completed_for_rust();
    let [PerformanceEntry::LongTask(task)] = entries.as_slice() else {
        panic!("one long task: {entries:?}");
    };
    assert_eq!(task.kind, JobKind::Timer);
    assert_eq!(task.thread, JsThreadName::Workers);
    assert_eq!(task.start, Timestamp(start));
    assert_eq!(task.duration(), Duration::from_millis(60));
    assert!(registry.take_completed_for_rust().is_empty(), "taken once");
}

#[test]
fn long_tasks_are_bounded_oldest_first() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::LONG_TASK);
    let start = ClockInstant::now();
    for millis in 0..1005 {
        assert!(registry.note_long_task(long_task(start, millis, JobKind::Command)));
    }
    let entries = registry.take_completed_for_rust();
    assert_eq!(entries.len(), 1000);
    let PerformanceEntry::LongTask(first) = &entries[0] else {
        panic!("a long task: {:?}", entries[0]);
    };
    assert_eq!(
        first.duration(),
        Duration::from_millis(5),
        "the five oldest went"
    );
}

#[test]
fn a_long_task_entry_writes_long_tasks_and_lynx_s_keys() {
    let origin = TimeOrigin::now();
    let entry = PerformanceEntry::LongTask(LongTaskEntry {
        start: at(&origin, 10),
        end: at(&origin, 72),
        thread: JsThreadName::Main,
        kind: JobKind::Command,
    });
    assert_eq!(entry.entry_type(), "longtask");
    assert_eq!(entry.name(), "command");
    let keys: Vec<_> = entry
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    assert_eq!(
        keys,
        [
            "entryType",
            "name",
            "startTime",
            "duration",
            "thread_name",
            "task_type",
            "task_name",
            "duration_ms"
        ]
    );
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["entryType"], "longtask");
    assert_eq!(value["name"], "command");
    assert_eq!(value["thread_name"], "Lynx_TASM");
    assert_eq!(value["task_type"], "native_func_task");
    assert_eq!(value["task_name"], "command");
    assert!((value["duration"].as_f64().unwrap() - 62.0).abs() < 1e-6);
    assert!((value["duration_ms"].as_f64().unwrap() - 62.0).abs() < 1e-6);
    let start = value["startTime"].as_f64().expect("a number");
    assert!((start - origin.unix_millis() - 10.0).abs() < 1e-6);
    // A kind Lynx files under no category leaves `task_type` out.
    let settle = PerformanceEntry::LongTask(LongTaskEntry {
        start: at(&origin, 0),
        end: at(&origin, 1),
        thread: JsThreadName::Workers,
        kind: JobKind::Settle,
    });
    let mut json = String::new();
    settle.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["thread_name"], "Lynx_JS");
    assert!(value.get("task_type").is_none());
}

fn blocking(threshold_millis: u64) -> JsBlockingConfig {
    JsBlockingConfig {
        threshold: Duration::from_millis(threshold_millis),
        report_interval: Duration::from_secs(5),
    }
}

fn blocking_entries(registry: &PipelineRegistry) -> Vec<JsBlockingEntry> {
    registry
        .take_completed_for_rust()
        .into_iter()
        .map(|entry| match entry {
            PerformanceEntry::JsBlocking(report) => report,
            entry => panic!("a js_blocking entry: {entry:?}"),
        })
        .collect()
}

#[test]
fn the_js_blocking_monitor_stamps_and_counts_only_while_on() {
    let registry = PipelineRegistry::detached();
    assert_eq!(registry.js_blocking_stamp(), None, "off by default");
    registry.note_js_blocking(Duration::from_secs(1));
    let config = JsBlockingConfig::default();
    assert_eq!(config.threshold, Duration::from_millis(20));
    assert_eq!(config.report_interval, Duration::from_secs(5));
    let epoch = registry.set_js_blocking_monitor(Some(config));
    assert!(epoch.is_some());
    assert!(registry.js_blocking_stamp().is_some());
    assert_eq!(registry.js_blocking_signal().borrow().anchor, None);
    assert_eq!(registry.set_js_blocking_monitor(None), None);
    assert_eq!(registry.js_blocking_stamp(), None);
    assert!(registry.js_blocking_signal().borrow().epoch > epoch.unwrap());
}

#[test]
fn js_blocking_reports_follow_lynx_s_formulas() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::JS_BLOCKING);
    let start = ClockInstant::now();
    registry.set_js_blocking_monitor_at(Some(blocking(20)), start);
    // Under the threshold: not counted.
    registry.note_js_blocking(Duration::from_millis(19));
    // Nothing counted: no report, and the span is not restarted.
    assert!(!registry.report_js_blocking_at(JsBlockingStage::LoadBundle, start));
    registry.note_js_blocking(Duration::from_millis(20));
    registry.note_js_blocking(Duration::from_millis(40));
    let load = start + Duration::from_millis(200);
    assert!(registry.report_js_blocking_at(JsBlockingStage::LoadBundle, load));
    let [report] = blocking_entries(&registry)[..] else {
        panic!("one report");
    };
    assert_eq!(report.stage, JsBlockingStage::LoadBundle);
    assert_eq!(report.total_blocking_time, Duration::from_millis(60));
    assert_eq!(report.total_blocking_count, 2);
    assert_eq!(report.total_duration, Duration::from_millis(200));
    assert!((report.blocking_ratio - 0.3).abs() < 1e-9);
    assert_eq!(report.avg_blocking_time, Duration::from_millis(30));
    assert_eq!(report.time_after_load_bundle, None);
    // Lynx resets the totals after a report: a timer report with nothing
    // counted since is not made.
    let first_timer = load + Duration::from_secs(5);
    assert!(!registry.report_js_blocking_at(JsBlockingStage::Timer, first_timer));
    registry.note_js_blocking(Duration::from_millis(100));
    let second_timer = load + Duration::from_secs(15);
    assert!(registry.report_js_blocking_at(JsBlockingStage::Timer, second_timer));
    let [report] = blocking_entries(&registry)[..] else {
        panic!("one report");
    };
    assert_eq!(report.stage, JsBlockingStage::Timer);
    assert_eq!(report.total_blocking_time, Duration::from_millis(100));
    assert_eq!(report.total_blocking_count, 1);
    // From the last report made, which the skipped one was not.
    assert_eq!(report.total_duration, Duration::from_secs(15));
    assert!((report.blocking_ratio - 0.1 / 15.0).abs() < 1e-9);
    assert_eq!(report.avg_blocking_time, Duration::from_millis(100));
    assert_eq!(report.time_after_load_bundle, Some(Duration::from_secs(15)));
}

#[test]
fn a_js_blocking_report_nobody_observes_is_made_and_dropped() {
    let registry = PipelineRegistry::detached();
    let start = ClockInstant::now();
    registry.set_js_blocking_monitor_at(Some(blocking(0)), start);
    registry.note_js_blocking(Duration::from_millis(5));
    assert!(!registry.report_js_blocking_at(JsBlockingStage::LoadBundle, start));
    assert_eq!(registry.generation(), 0);
    // The report was made: its totals were spent.
    registry.observe_rust(EntryTypes::JS_BLOCKING);
    assert!(!registry.report_js_blocking_at(JsBlockingStage::Timer, start));
    assert!(registry.take_completed_for_rust().is_empty());
}

#[test]
fn the_page_load_makes_the_load_bundle_report_and_anchors_the_timer() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::JS_BLOCKING);
    let epoch = registry
        .set_js_blocking_monitor(Some(blocking(0)))
        .expect("on");
    registry.note_js_blocking(Duration::from_millis(1));
    registry.open_load(OpenerId::PAGE, "app:///main.js", None);
    assert!(registry.mark_load(OpenerId::PAGE, LoadMark::MtsRenderStart));
    let mut binding = flush(OpenerId::PAGE, 0, &[], 1);
    binding.load = true;
    registry.bind_commit(binding);
    registry.note_presented(frame(1));
    let signal = *registry.js_blocking_signal().borrow();
    assert_eq!(signal.epoch, epoch);
    assert!(signal.anchor.is_some(), "the timer starts at the load");
    let [report] = blocking_entries(&registry)[..] else {
        panic!("the loadBundle report");
    };
    assert_eq!(report.stage, JsBlockingStage::LoadBundle);
    assert_eq!(report.total_blocking_count, 1);
    // A report task of an earlier configuration reports nothing.
    registry.note_js_blocking(Duration::from_millis(1));
    assert!(!registry.report_js_blocking(epoch + 1));
    assert!(registry.report_js_blocking(epoch));
    let [report] = blocking_entries(&registry)[..] else {
        panic!("the timer report");
    };
    assert_eq!(report.stage, JsBlockingStage::Timer);
    assert!(report.time_after_load_bundle.is_some());
    // Turned on again after the load: no `loadBundle` report, the timer
    // anchored at once.
    registry.set_js_blocking_monitor(None);
    let again = registry
        .set_js_blocking_monitor(Some(blocking(0)))
        .expect("on");
    let signal = *registry.js_blocking_signal().borrow();
    assert_eq!(signal.epoch, again);
    assert!(signal.anchor.is_some());
}

#[test]
fn a_js_blocking_entry_writes_lynx_s_keys_and_types() {
    let origin = TimeOrigin::now();
    let entry = PerformanceEntry::JsBlocking(JsBlockingEntry {
        stage: JsBlockingStage::Timer,
        total_blocking_time: Duration::from_millis(45),
        total_blocking_count: 2,
        total_duration: Duration::from_secs(5),
        blocking_ratio: 0.009,
        avg_blocking_time: Duration::from_micros(22_500),
        time_after_load_bundle: Some(Duration::from_millis(5_001)),
    });
    assert_eq!(entry.entry_type(), "js_blocking");
    assert_eq!(entry.name(), "js_blocking");
    let keys: Vec<_> = entry
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    assert_eq!(
        keys,
        [
            "entryType",
            "name",
            "stage",
            "total_blocking_time",
            "total_blocking_count",
            "total_duration",
            "blocking_ratio",
            "avg_blocking_time",
            "time_after_load_bundle"
        ]
    );
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["entryType"], "js_blocking");
    assert_eq!(value["name"], "js_blocking");
    assert_eq!(value["stage"], "timer");
    assert_eq!(value["total_blocking_time"].as_i64(), Some(45));
    assert_eq!(value["total_blocking_count"].as_i64(), Some(2));
    assert_eq!(value["total_duration"].as_i64(), Some(5_000));
    assert_eq!(value["time_after_load_bundle"].as_i64(), Some(5_001));
    assert!((value["blocking_ratio"].as_f64().unwrap() - 0.009).abs() < 1e-12);
    assert!((value["avg_blocking_time"].as_f64().unwrap() - 22.5).abs() < 1e-9);
    // A `loadBundle` report has no `time_after_load_bundle`, and a ratio
    // over a zero span is `null` rather than invalid JSON.
    let load = PerformanceEntry::JsBlocking(JsBlockingEntry {
        stage: JsBlockingStage::LoadBundle,
        total_duration: Duration::ZERO,
        blocking_ratio: f64::INFINITY,
        time_after_load_bundle: None,
        ..match entry {
            PerformanceEntry::JsBlocking(report) => report,
            _ => unreachable!(),
        }
    });
    let mut json = String::new();
    load.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["stage"], "loadBundle");
    assert!(value["blocking_ratio"].is_null());
    assert!(value.get("time_after_load_bundle").is_none());
}

#[test]
fn every_type_includes_the_two_monitors() {
    assert!(EntryTypes::ALL.contains(EntryTypes::LONG_TASK));
    assert!(EntryTypes::ALL.contains(EntryTypes::JS_BLOCKING));
    assert!(!EntryTypes::LONG_TASK.intersects(EntryTypes::JS_BLOCKING));
}
