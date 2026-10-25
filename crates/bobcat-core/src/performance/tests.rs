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
    registry.observe_worker(worker, Some(EntryTypes::NONE));
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
    registry.observe_worker(worker, Some(EntryTypes::PAINT));
    registry.observe_worker(other, Some(EntryTypes::NONE));
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
    registry.observe_worker(WorkerKey::new(1), Some(EntryTypes::NONE));
    assert!(!registry.wants_content_detection());
    // Asking for paint turns detection on until the contentful paint.
    registry.observe_worker(WorkerKey::new(1), Some(EntryTypes::PAINT));
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

#[test]
fn every_type_includes_memory() {
    assert!(EntryTypes::ALL.contains(EntryTypes::MEMORY));
    assert!(!EntryTypes::MEMORY.intersects(EntryTypes::JS_BLOCKING | EntryTypes::LONG_TASK));
}

fn memory(threshold: u64) -> MemoryConfig {
    MemoryConfig {
        change_threshold: threshold,
        ..MemoryConfig::default()
    }
}

fn reading(category: MemoryCategory, size_bytes: u64) -> MemoryItem {
    MemoryItem {
        category,
        size_bytes,
        instance_count: 1,
        detail: vec![(Arc::from("key"), Arc::from(size_bytes.to_string()))],
    }
}

fn memory_entries(registry: &PipelineRegistry) -> Vec<MemoryEntry> {
    registry
        .take_completed_for_rust()
        .into_iter()
        .filter_map(|entry| match entry {
            PerformanceEntry::Memory(memory) => Some(memory),
            _ => None,
        })
        .collect()
}

#[test]
fn the_memory_monitor_reads_nothing_while_off() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::ALL);
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 10)));
    assert!(!registry.take_frame_memory_sample());
    registry.request_frame_memory_sample();
    let epoch = registry.set_memory_monitor(Some(memory(0))).expect("on");
    assert!(registry.memory_sampling(epoch));
    assert_eq!(registry.set_memory_monitor(None), None);
    assert!(!registry.memory_sampling(epoch));
    assert!(!registry.memory_sampling(epoch + 1));
    assert!(
        !registry.take_frame_memory_sample(),
        "turning the monitor off withdraws the painter's request"
    );
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 10)));
    assert!(memory_entries(&registry).is_empty());
    assert_eq!(registry.generation(), 0);
}

#[test]
fn the_frame_sample_request_is_taken_once() {
    let registry = PipelineRegistry::detached();
    assert!(!registry.take_frame_memory_sample());
    registry.request_frame_memory_sample();
    assert!(registry.take_frame_memory_sample());
    assert!(!registry.take_frame_memory_sample());
}

#[test]
fn a_zero_threshold_reports_every_reading_that_changes_a_category() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::MEMORY);
    registry.set_memory_monitor(Some(memory(0)));
    assert!(registry.note_memory(reading(MemoryCategory::MainThreadScriptingEngine, 100)));
    assert!(registry.note_memory(reading(MemoryCategory::LynxTasmElement, 20)));
    assert!(registry.note_memory(reading(MemoryCategory::MainThreadScriptingEngine, 101)));
    // Lynx's `UpdateMemoryUsage`: a category whose size did not change is
    // not a report, and its reading is not replaced.
    let mut same = reading(MemoryCategory::LynxTasmElement, 20);
    same.detail = Vec::new();
    assert!(!registry.note_memory(same));
    let entries = memory_entries(&registry);
    let totals: Vec<u64> = entries.iter().map(|entry| entry.size_bytes).collect();
    assert_eq!(totals, [100, 120, 121]);
    let last = entries.last().expect("three reports");
    let categories: Vec<MemoryCategory> = last.items.iter().map(|item| item.category).collect();
    assert_eq!(
        categories,
        [
            MemoryCategory::MainThreadScriptingEngine,
            MemoryCategory::LynxTasmElement
        ]
    );
    let elements = last.item(MemoryCategory::LynxTasmElement).expect("read");
    assert_eq!(
        elements.detail.len(),
        1,
        "the unchanged reading was kept out"
    );
}

#[test]
fn a_threshold_holds_back_a_small_change_and_passes_a_large_one() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::MEMORY);
    registry.set_memory_monitor(Some(memory(1_000)));
    // The first reading is reported whatever it changed.
    assert!(registry.note_memory(reading(MemoryCategory::MainThreadScriptingEngine, 10)));
    // Kept, not reported: the total moved by less than the threshold.
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 500)));
    assert!(!registry.note_memory(reading(MemoryCategory::CommittedFrame, 400)));
    // This one takes the total 1 000 past the last report's.
    assert!(registry.note_memory(reading(
        MemoryCategory::BackgroundThreadScriptingEngine,
        100
    )));
    // A drop counts as much as a rise, measured from the last report.
    assert!(!registry.note_memory(reading(MemoryCategory::CommittedFrame, 0)));
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 0)));
    assert!(registry.note_memory(reading(MemoryCategory::BackgroundThreadScriptingEngine, 0)));
    let entries = memory_entries(&registry);
    let totals: Vec<u64> = entries.iter().map(|entry| entry.size_bytes).collect();
    assert_eq!(totals, [10, 1_010, 10]);
    assert_eq!(entries[1].items.len(), 4, "every category's latest reading");
    // A new configuration starts afresh: its first reading is forced again.
    registry.set_memory_monitor(Some(memory(1_000)));
    assert!(registry.note_memory(reading(MemoryCategory::LynxTasmElement, 1)));
    let [entry] = &memory_entries(&registry)[..] else {
        panic!("the forced first reading");
    };
    assert_eq!(entry.items.len(), 1);
}

#[test]
fn memory_reports_reach_the_embedder_and_the_workers_that_ask() {
    let registry = PipelineRegistry::detached();
    let asking = WorkerKey::new(3);
    let other = WorkerKey::new(4);
    registry.observe_worker(asking, Some(EntryTypes::MEMORY));
    registry.observe_worker(other, Some(EntryTypes::NONE));
    registry.set_memory_monitor(Some(memory(0)));
    // Nobody asks the Rust side; the asking worker is owed it.
    assert!(registry.note_memory(reading(MemoryCategory::MainThreadScriptingEngine, 5)));
    assert_eq!(registry.take_wakes(), vec![asking]);
    assert!(memory_entries(&registry).is_empty());
    assert!(registry.take_completed_for_worker(other).is_empty());
    let entries = registry.take_completed_for_worker(asking);
    let [PerformanceEntry::Memory(entry)] = &entries[..] else {
        panic!("one memory entry: {entries:?}");
    };
    assert_eq!(entry.size_bytes, 5);
    // A worker that stops asking drops what it had not taken.
    assert!(registry.note_memory(reading(MemoryCategory::MainThreadScriptingEngine, 6)));
    registry.observe_worker(asking, Some(EntryTypes::NONE));
    assert!(registry.take_completed_for_worker(asking).is_empty());
    // Nobody asks at all: kept as the latest reading, reported to no one.
    let generation = registry.generation();
    assert!(!registry.note_memory(reading(MemoryCategory::MainThreadScriptingEngine, 7)));
    assert_eq!(registry.generation(), generation);
}

fn memory_entry() -> PerformanceEntry {
    PerformanceEntry::Memory(MemoryEntry {
        size_bytes: 4_736,
        items: vec![
            MemoryItem {
                category: MemoryCategory::MainThreadScriptingEngine,
                size_bytes: 4_096,
                instance_count: 1,
                detail: vec![
                    (Arc::from("obj_count"), Arc::from("12")),
                    (Arc::from("sharedRuntime"), Arc::from("thread")),
                ],
            },
            MemoryItem {
                category: MemoryCategory::LynxTasmElement,
                size_bytes: 640,
                instance_count: 2,
                detail: vec![
                    (Arc::from("elementCount"), Arc::from("2")),
                    (Arc::from("singleElementSizeBytes"), Arc::from("320")),
                ],
            },
        ],
    })
}

#[test]
fn a_memory_entry_writes_lynx_s_nested_maps() {
    let origin = TimeOrigin::now();
    let entry = memory_entry();
    assert_eq!(entry.entry_type(), "memory");
    assert_eq!(entry.name(), "memory");
    assert!(entry.pipeline().is_none());
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(
        value,
        serde_json::json!({
            "entryType": "memory",
            "name": "memory",
            "detail": {
                "mainThreadScriptingEngine": {
                    "category": "mainThreadScriptingEngine",
                    "sizeBytes": 4_096,
                    "instanceCount": 1,
                    "detail": { "obj_count": "12", "sharedRuntime": "thread" },
                },
                "lynxTasmElement": {
                    "category": "lynxTasmElement",
                    "sizeBytes": 640,
                    "instanceCount": 2,
                    "detail": { "elementCount": "2", "singleElementSizeBytes": "320" },
                },
            },
            "sizeBytes": 4_736,
        })
    );
    // `detail` comes before the total, as `ReportMemory` pushes it.
    assert!(json.find("\"detail\"") < json.find(",\"sizeBytes\":4736}"));
}

/// The fields of a record whose text is ASCII, where a UTF-16 count is a
/// byte count.
fn ascii_fields(mut record: &str) -> Vec<String> {
    let mut fields = Vec::new();
    while let Some((count, rest)) = record.split_once(':') {
        let length: usize = count.parse().expect("a field length");
        fields.push(rest[..length].to_owned());
        record = &rest[length..];
    }
    fields
}

#[test]
fn the_record_encoding_carries_a_memory_entry_s_key_paths() {
    let origin = TimeOrigin::now();
    let record = encode_entries(&[memory_entry()], &origin);
    assert!(record.is_ascii());
    let fields = ascii_fields(&record);
    let pairs: Vec<(&str, &str)> = fields[1..]
        .chunks(2)
        .map(|pair| (pair[0].as_str(), pair[1].as_str()))
        .collect();
    assert_eq!(fields[0], pairs.len().to_string());
    assert_eq!(pairs[0], ("entryType", "smemory"));
    assert!(pairs.contains(&("detail.lynxTasmElement.sizeBytes", "n640")));
    assert!(pairs.contains(&("detail.lynxTasmElement.instanceCount", "n2")));
    assert!(pairs.contains(&(
        "detail.mainThreadScriptingEngine.detail.sharedRuntime",
        "sthread"
    )));
    assert_eq!(pairs.last(), Some(&("sizeBytes", "n4736")));
}

#[test]
fn a_framework_mark_with_a_dot_stays_one_member() {
    let origin = TimeOrigin::now();
    let mut timing = pipeline(&origin);
    timing.framework_rendering_timing.marks = vec![
        (Arc::from("a.b"), at(&origin, 1)),
        (Arc::from("c\\d"), at(&origin, 2)),
    ];
    let entry = PerformanceEntry::Pipeline(Box::new(timing));
    let keys: Vec<String> = entry
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    assert!(keys.contains(&"frameworkRenderingTiming.a\\.b".to_owned()));
    assert!(keys.contains(&"frameworkRenderingTiming.c\\\\d".to_owned()));
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert!(value["frameworkRenderingTiming"]["a.b"].is_number());
    assert!(value["frameworkRenderingTiming"]["c\\d"].is_number());
    assert!(value["frameworkRenderingTiming"].get("a").is_none());
}

fn lazy_bundle(origin: &TimeOrigin, url: &str, success: bool) -> LazyBundleEntry {
    LazyBundleEntry {
        component_url: Arc::from(url),
        size_bytes: success.then_some(2048),
        load_success: success,
        require: at(origin, 1)..at(origin, 5),
        decode: success.then(|| at(origin, 6)..at(origin, 8)),
    }
}

#[test]
fn every_type_includes_resource() {
    assert!(EntryTypes::ALL.contains(EntryTypes::RESOURCE));
}

#[test]
fn a_fetch_is_timed_only_while_someone_observes_resource() {
    let registry = PipelineRegistry::detached();
    assert!(!registry.times_resources(), "nobody observes at first");
    registry.observe_rust(EntryTypes::ALL);
    assert!(registry.times_resources());
    registry.observe_rust(EntryTypes::PIPELINE | EntryTypes::MEMORY);
    assert!(!registry.times_resources(), "other types time no fetch");
    let worker = WorkerKey::new(3);
    registry.observe_worker(worker, Some(EntryTypes::NONE));
    assert!(!registry.times_resources());
    registry.observe_worker(worker, Some(EntryTypes::RESOURCE));
    assert!(registry.times_resources(), "a BTS observer of resource");
    registry.forget_worker(worker);
    assert!(!registry.times_resources(), "the worker ended");
}

#[test]
fn a_lazy_bundle_reaches_the_embedder_and_the_bts_of_the_realm_that_fetched() {
    let registry = PipelineRegistry::detached();
    let origin = registry.time_origin();
    registry.observe_rust(EntryTypes::RESOURCE);
    let page_bts = WorkerKey::new(1);
    let card_bts = WorkerKey::new(2);
    let card = registry.allocate_opener();
    registry.bind_worker(page_bts, OpenerId::PAGE);
    registry.bind_worker(card_bts, card);
    registry.observe_worker(page_bts, Some(EntryTypes::RESOURCE));
    registry.observe_worker(card_bts, Some(EntryTypes::NONE));

    // The page realm's fetch: the embedder and the page's BTS.
    assert!(registry.note_lazy_bundle(
        Opener::Realm(OpenerId::PAGE),
        lazy_bundle(&origin, "app:///page.bundle", true),
    ));
    // The BTS's own fetch: the same audiences.
    assert!(registry.note_lazy_bundle(
        Opener::Worker(page_bts),
        lazy_bundle(&origin, "app:///bts.bundle", true),
    ));
    // A card's fetch: its BTS does not ask for `resource`, so the embedder
    // alone.
    assert!(registry.note_lazy_bundle(
        Opener::Realm(card),
        lazy_bundle(&origin, "app:///card.bundle", false),
    ));
    assert_eq!(registry.take_wakes(), vec![page_bts]);

    let urls = |entries: Vec<PerformanceEntry>| -> Vec<String> {
        entries
            .into_iter()
            .map(|entry| match entry {
                PerformanceEntry::LazyBundle(bundle) => bundle.component_url.to_string(),
                other => panic!("a lazy bundle: {other:?}"),
            })
            .collect()
    };
    assert_eq!(
        urls(registry.take_completed_for_rust()),
        [
            "app:///page.bundle",
            "app:///bts.bundle",
            "app:///card.bundle"
        ]
    );
    assert_eq!(
        urls(registry.take_completed_for_worker(page_bts)),
        ["app:///page.bundle", "app:///bts.bundle"]
    );
    assert!(registry.take_completed_for_worker(card_bts).is_empty());
    assert!(registry.take_completed_for_rust().is_empty(), "taken once");
}

#[test]
fn a_lazy_bundle_nobody_asks_for_is_dropped() {
    let registry = PipelineRegistry::detached();
    let origin = registry.time_origin();
    registry.observe_rust(EntryTypes::PIPELINE);
    assert!(!registry.note_lazy_bundle(
        Opener::Realm(OpenerId::PAGE),
        lazy_bundle(&origin, "app:///lazy.bundle", true),
    ));
    assert_eq!(registry.generation(), 0);
    assert!(registry.take_completed_for_rust().is_empty());
}

#[test]
fn a_fetch_s_entry_takes_the_fetcher_s_readings_and_falls_back_to_its_delivery() {
    let origin = TimeOrigin::now();
    let started = origin.instant + Duration::from_millis(1);
    let received = origin.instant + Duration::from_millis(4);
    let delivered = origin.instant + Duration::from_millis(9);
    let installed =
        (origin.instant + Duration::from_millis(5))..(origin.instant + Duration::from_millis(7));
    let outcome = crate::resource::FetchOutcome {
        size_bytes: Some(512),
        received: Some(received),
        installed: Some(installed.clone()),
    };
    let fetched = LazyBundleEntry::from_fetch(
        Arc::from("app:///a.bundle"),
        started,
        delivered,
        Some(&outcome),
    );
    assert!(fetched.load_success);
    assert_eq!(fetched.size_bytes, Some(512));
    assert_eq!(fetched.require, Timestamp(started)..Timestamp(received));
    assert_eq!(
        fetched.decode,
        Some(Timestamp(installed.start)..Timestamp(installed.end))
    );

    // A fetcher that read nothing: the delivery ends the require span.
    let silent = crate::resource::FetchOutcome::default();
    let quiet = LazyBundleEntry::from_fetch(
        Arc::from("app:///a.bundle"),
        started,
        delivered,
        Some(&silent),
    );
    assert!(quiet.load_success);
    assert_eq!(quiet.require, Timestamp(started)..Timestamp(delivered));
    assert_eq!((quiet.size_bytes, quiet.decode), (None, None));

    // A failed fetch carries no outcome.
    let failed =
        LazyBundleEntry::from_fetch(Arc::from("app:///b.bundle"), started, delivered, None);
    assert!(!failed.load_success);
    assert_eq!(failed.require, Timestamp(started)..Timestamp(delivered));
    assert_eq!((failed.size_bytes, failed.decode), (None, None));
}

#[test]
fn a_lazy_bundle_entry_writes_lynx_s_keys_and_types() {
    let origin = TimeOrigin::now();
    let entry = PerformanceEntry::LazyBundle(lazy_bundle(&origin, "app:///lazy.bundle", true));
    assert_eq!(entry.entry_type(), "resource");
    assert_eq!(entry.name(), "lazyBundle");
    assert!(entry.pipeline().is_none());
    let keys: Vec<_> = entry
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    assert_eq!(
        keys,
        [
            "entryType",
            "name",
            "componentUrl",
            "size",
            "loadSuccess",
            "mode",
            "decodeStart",
            "decodeEnd",
            "requireStart",
            "requireEnd",
        ]
    );
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["componentUrl"], "app:///lazy.bundle");
    assert_eq!(value["size"], 2048);
    assert_eq!(value["loadSuccess"], true);
    assert_eq!(value["mode"], "normal");
    assert!(value.get("sync").is_none(), "native's writer leaves it out");
    let start = value["requireStart"].as_f64().expect("a number");
    let end = value["requireEnd"].as_f64().expect("a number");
    assert!((end - start - 4.0).abs() < 1e-6, "{json}");

    // A failed fetch: no size and no decode, `loadSuccess` false.
    let failed = PerformanceEntry::LazyBundle(lazy_bundle(&origin, "app:///gone.bundle", false));
    let keys: Vec<_> = failed
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    assert_eq!(
        keys,
        [
            "entryType",
            "name",
            "componentUrl",
            "loadSuccess",
            "mode",
            "requireStart",
            "requireEnd",
        ]
    );
    let record = encode_entries(&[failed], &origin);
    let fields = ascii_fields(&record);
    let pairs: Vec<(&str, &str)> = fields[1..]
        .chunks(2)
        .map(|pair| (pair[0].as_str(), pair[1].as_str()))
        .collect();
    assert!(pairs.contains(&("loadSuccess", "b0")), "{pairs:?}");
    assert!(pairs.contains(&("entryType", "sresource")));
}

/// Opens, renders and presents the load of `realm`, whose bundle is `url`,
/// at commit `commit_id`.
fn complete_load(registry: &PipelineRegistry, realm: OpenerId, url: &str, commit_id: u64) {
    registry.open_load(realm, url, None);
    registry.mark_load(realm, LoadMark::VmExecuteEnd);
    assert!(registry.mark_load(realm, LoadMark::MtsRenderStart));
    let mut binding = flush(realm, 0, &[], commit_id);
    binding.load = true;
    registry.bind_commit(binding);
    registry.note_presented(frame(commit_id));
}

#[test]
fn a_card_s_load_is_owed_as_loadmetrics_whoever_observes_and_the_page_s_is_not() {
    let registry = PipelineRegistry::detached();
    complete_load(&registry, OpenerId::PAGE, "app:///main.js", 1);
    assert!(
        registry.take_frame_loads().is_empty(),
        "the page's own load"
    );
    assert_eq!(registry.generation(), 0, "nobody observes the page's load");

    let card = registry.allocate_opener();
    complete_load(&registry, card, "app:///child.web.bundle", 2);
    assert!(registry.generation() > 0, "the embedder's pump looks");
    let owed = registry.take_frame_loads();
    let [(owner, load)] = owed.as_slice() else {
        panic!("one card load: {owed:?}");
    };
    assert_eq!(*owner, card);
    assert_eq!(&*load.url, "app:///child.web.bundle");
    assert_eq!(load.pipeline.origin, PipelineOrigin::LoadBundle);
    assert!(load.load_bundle.start <= load.load_bundle.end);
    assert!(registry.take_frame_loads().is_empty(), "owed once");
    assert!(
        registry.take_completed_for_rust().is_empty(),
        "and reported to nobody else"
    );

    // Sent again after `set_extra_timing`, a load is not owed again.
    let extra = ExtraTiming {
        open_time: Some(SystemTime::now()),
        ..ExtraTiming::default()
    };
    registry.observe_rust(EntryTypes::LOAD_BUNDLE);
    registry.set_extra_timing(&extra);
    assert!(registry.take_frame_loads().is_empty());
}

#[test]
fn every_type_includes_container() {
    assert!(EntryTypes::ALL.contains(EntryTypes::CONTAINER));
    assert!(!EntryTypes::CONTAINER.intersects(
        EntryTypes::PIPELINE
            | EntryTypes::LOAD_BUNDLE
            | EntryTypes::PAINT
            | EntryTypes::LONG_TASK
            | EntryTypes::JS_BLOCKING
            | EntryTypes::MEMORY
            | EntryTypes::RESOURCE
    ));
}

fn node(bits: u64) -> NodeId {
    NodeId::from_bits(bits).expect("a non-zero handle")
}

/// A report of root `root` at a device pixel ratio of 2: a 40×20 CSS px box
/// at (10, 20), 600 square CSS px of it painted, first rendered 2 ms after
/// `origin` and grown at 5 ms.
fn container_paint(origin: &TimeOrigin, root: u64) -> terminus::link::ContainerPaint {
    use terminus::link::{Point2D, Size2D};
    terminus::link::ContainerPaint {
        root: node(root),
        commit_id: 3,
        painted: Rect::new(Point2D::new(20.0, 40.0), Size2D::new(80.0, 40.0)),
        size: 2400.0,
        first_render: at(origin, 2).instant(),
        latest: at(origin, 5).instant(),
        last_painted: node(root + 1),
        device_pixel_ratio: 2.0,
    }
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "whole CSS px halved from whole device px are exact"
)]
fn a_container_entry_writes_the_draft_s_names_in_css_px() {
    let origin = TimeOrigin::now();
    let entry = ContainerEntry::from_paint(&container_paint(&origin, 5), Arc::from("hero"));
    assert_eq!(entry.root, node(5));
    assert_eq!(entry.last_painted, node(6));
    assert_eq!(
        entry.intersection_rect,
        Rect::new(
            terminus::link::Point2D::new(10.0, 20.0),
            terminus::link::Size2D::new(40.0, 20.0)
        )
    );
    assert!((entry.size - 600.0).abs() < f32::EPSILON);
    let entry = PerformanceEntry::Container(entry);
    assert_eq!(entry.entry_type(), "container");
    assert_eq!(entry.name(), "", "the draft's `name` is empty");
    assert!(entry.pipeline().is_none());
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
            "identifier",
            "intersectionRect.x",
            "intersectionRect.y",
            "intersectionRect.width",
            "intersectionRect.height",
            "size",
            "firstRenderTime",
        ]
    );
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["entryType"], "container");
    assert_eq!(value["name"], "");
    assert_eq!(value["identifier"], "hero");
    assert_eq!(value["duration"], 0);
    let rect = &value["intersectionRect"];
    let member = |name: &str| rect[name].as_f64().expect("a number");
    assert_eq!(
        [member("x"), member("y"), member("width"), member("height")],
        [10.0, 20.0, 40.0, 20.0],
        "{json}"
    );
    assert_eq!(value["size"].as_f64(), Some(600.0));
    assert!(value.get("rootElement").is_none());
    assert!(value.get("lastPaintedElement").is_none());
    let start = value["startTime"].as_f64().expect("a number");
    let first = value["firstRenderTime"].as_f64().expect("a number");
    assert!((start - first - 3.0).abs() < 1e-6, "{json}");

    // The record the BTS decodes nests `intersectionRect` on its `.`s.
    let record = encode_entries(&[entry], &origin);
    let fields = ascii_fields(&record);
    let pairs: Vec<(&str, &str)> = fields[1..]
        .chunks(2)
        .map(|pair| (pair[0].as_str(), pair[1].as_str()))
        .collect();
    assert!(pairs.contains(&("name", "s")), "{pairs:?}");
    assert!(
        pairs.contains(&("intersectionRect.width", "n40")),
        "{pairs:?}"
    );
}

#[test]
fn container_reports_reach_the_embedder_and_the_workers_that_ask() {
    let origin = TimeOrigin::now();
    let registry = PipelineRegistry::detached();
    registry.set_container_identifier(node(5), Some("hero"));

    // Nobody observes `container`: the reports are dropped unlocked.
    let generation = registry.generation();
    assert!(
        registry
            .note_container_paints(&[container_paint(&origin, 5)])
            .is_empty()
    );
    assert_eq!(registry.generation(), generation);

    let asking = WorkerKey::new(3);
    let other = WorkerKey::new(4);
    registry.observe_worker(asking, Some(EntryTypes::CONTAINER));
    registry.observe_worker(other, Some(EntryTypes::PAINT));
    registry.observe_rust(EntryTypes::CONTAINER);
    let owed =
        registry.note_container_paints(&[container_paint(&origin, 5), container_paint(&origin, 9)]);
    assert_eq!(owed, vec![asking]);
    assert!(registry.generation() > generation);
    let rust = registry.take_completed_for_rust();
    let identifiers: Vec<&str> = rust
        .iter()
        .map(|entry| match entry {
            PerformanceEntry::Container(container) => &*container.identifier,
            other => panic!("a container entry: {other:?}"),
        })
        .collect();
    // A root with no identifier recorded is named by the empty string.
    assert_eq!(identifiers, ["hero", ""]);
    assert_eq!(registry.take_completed_for_worker(asking).len(), 2);
    assert!(registry.take_completed_for_worker(other).is_empty());

    // A removed attribute names nothing; a worker that stops asking drops
    // what it had not taken.
    registry.set_container_identifier(node(5), None);
    registry.observe_rust(EntryTypes::NONE);
    registry.note_container_paints(&[container_paint(&origin, 5)]);
    registry.observe_worker(asking, Some(EntryTypes::NONE));
    assert!(registry.take_completed_for_worker(asking).is_empty());
    assert!(registry.take_completed_for_rust().is_empty());
}

#[test]
fn the_container_stop_is_taken_once() {
    let registry = PipelineRegistry::detached();
    assert!(!registry.take_container_stop());
    registry.request_container_stop();
    assert!(registry.take_container_stop());
    assert!(!registry.take_container_stop());
}
