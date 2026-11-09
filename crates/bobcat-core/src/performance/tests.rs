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

#[test]
fn a_delivered_entry_reaches_the_audiences_that_asked_for_its_type_in_order() {
    let registry = PipelineRegistry::detached();
    let (painting, plain) = (WorkerKey::new(1), WorkerKey::new(2));
    registry.observe_worker(painting, Some(EntryTypes::PAINT));
    registry.observe_worker(plain, Some(EntryTypes::NONE));
    let start = ClockInstant::now();
    let paint = |millis| {
        PerformanceEntry::Paint(PaintEntry {
            name: PaintName::FirstPaint,
            start: Timestamp(start + Duration::from_millis(millis)),
        })
    };
    // Nobody asked: nothing kept, nothing to wake.
    assert!(!registry.deliver_entry(paint(0), EntryTypes::PAINT, WorkerAudience::None));
    assert!(!registry.deliver_entry(paint(0), EntryTypes::PAINT, WorkerAudience::One(plain)));
    assert_eq!(registry.generation(), 0);
    assert!(registry.deliver_entry(paint(1), EntryTypes::PAINT, WorkerAudience::Every));
    assert!(registry.deliver_entry(paint(2), EntryTypes::PAINT, WorkerAudience::One(painting)));
    assert_eq!(registry.take_wakes(), vec![painting]);
    assert_eq!(
        registry.take_completed_for_worker(painting),
        vec![paint(1), paint(2)],
        "in production order"
    );
    assert!(registry.take_completed_for_worker(plain).is_empty());
    assert!(registry.take_completed_for_rust().is_empty());
    // A worker that stops observing drops what it had not taken.
    assert!(registry.deliver_entry(paint(3), EntryTypes::PAINT, WorkerAudience::Every));
    registry.observe_worker(painting, None);
    assert!(registry.take_completed_for_worker(painting).is_empty());
}

#[test]
fn the_long_task_monitor_is_off_by_default_and_reports_to_rust_alone_bounded() {
    let registry = PipelineRegistry::detached();
    assert_eq!(registry.long_task_threshold(), None);
    let config = LongTaskConfig::default();
    assert_eq!(config.threshold, Duration::from_millis(50));
    registry.set_long_task_threshold(Some(Duration::ZERO));
    assert_eq!(registry.long_task_threshold(), Some(Duration::ZERO));
    registry.set_long_task_threshold(None);
    assert_eq!(registry.long_task_threshold(), None);
    assert!(EntryTypes::ALL.contains(EntryTypes::LONG_TASK | EntryTypes::JS_BLOCKING));
    let start = ClockInstant::now();
    let deliver = |millis, kind| {
        registry.deliver_entry(
            PerformanceEntry::LongTask(LongTaskEntry {
                start: Timestamp(start),
                end: Timestamp(start + Duration::from_millis(millis)),
                thread: JsThreadName::Workers,
                kind,
            }),
            EntryTypes::LONG_TASK,
            WorkerAudience::None,
        )
    };
    // Nobody observes long tasks, a worker included: dropped as it is made.
    registry.observe_rust(EntryTypes::PIPELINE);
    registry.observe_worker(WorkerKey::new(1), Some(EntryTypes::ALL));
    assert!(!deliver(60, JobKind::Post));
    assert_eq!(registry.generation(), 0);
    registry.observe_rust(EntryTypes::LONG_TASK);
    for millis in 0..1005 {
        assert!(deliver(millis, JobKind::Timer));
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
    assert!(registry.take_completed_for_rust().is_empty(), "taken once");
    assert_eq!(registry.take_completed_for_worker(WorkerKey::new(1)), []);
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
    assert_eq!((entry.entry_type(), entry.name()), ("longtask", "command"));
    let keys: Vec<_> = entry
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    let expected = "entryType name startTime duration thread_name task_type task_name duration_ms";
    assert_eq!(keys.join(" "), expected);
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
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
fn the_js_blocking_monitor_is_off_by_default_and_reports_to_rust_alone_bounded() {
    let registry = PipelineRegistry::detached();
    assert_eq!(registry.js_blocking_stamp(), None, "off by default");
    let config = JsBlockingConfig::default();
    assert_eq!(config.threshold, Duration::from_millis(20));
    assert_eq!(config.report_interval, Duration::from_secs(5));
    let start = ClockInstant::now();
    // Off: a wait counts nothing, and nothing reports.
    registry.note_js_blocking(Duration::from_secs(1));
    assert!(!registry.report_js_blocking_at(JsBlockingStage::LoadBundle, start));
    let epoch = registry
        .set_js_blocking_monitor(Some(blocking(0)), start)
        .expect("on");
    assert!(registry.js_blocking_stamp().is_some());
    assert!(registry.js_blocking_is_current(epoch));
    assert_eq!(registry.js_blocking_anchor(epoch), None, "no load yet");
    // A report nobody observes is made, its totals spent, and dropped; a
    // worker that asked for everything is not its audience.
    registry.observe_worker(WorkerKey::new(1), Some(EntryTypes::ALL));
    registry.note_js_blocking(Duration::from_millis(5));
    assert!(!registry.report_js_blocking_at(JsBlockingStage::LoadBundle, start));
    registry.observe_rust(EntryTypes::JS_BLOCKING);
    assert!(!registry.report_js_blocking_at(JsBlockingStage::Timer, start));
    assert_eq!(registry.generation(), 0);
    for _ in 0..1005 {
        registry.note_js_blocking(Duration::from_millis(1));
        assert!(registry.report_js_blocking_at(JsBlockingStage::Timer, start));
    }
    assert_eq!(blocking_entries(&registry).len(), 1000);
    assert_eq!(registry.take_completed_for_worker(WorkerKey::new(1)), []);
    assert_eq!(registry.set_js_blocking_monitor(None, start), None);
    assert_eq!(registry.js_blocking_stamp(), None);
    assert!(!registry.js_blocking_is_current(epoch));
}

#[test]
fn js_blocking_reports_follow_lynx_s_formulas() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::JS_BLOCKING);
    let start = ClockInstant::now();
    registry.set_js_blocking_monitor(Some(blocking(20)), start);
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
    assert_eq!(report.total_blocking_count, 1);
    // From the last report made, which the skipped one was not.
    assert_eq!(report.total_duration, Duration::from_secs(15));
    assert!((report.blocking_ratio - 0.1 / 15.0).abs() < 1e-9);
    assert_eq!(report.avg_blocking_time, Duration::from_millis(100));
    assert_eq!(report.time_after_load_bundle, Some(Duration::from_secs(15)));
}

#[test]
fn the_page_load_makes_the_load_bundle_report_and_anchors_the_timer() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::JS_BLOCKING);
    let epoch = registry
        .set_js_blocking_monitor(Some(blocking(0)), ClockInstant::now())
        .expect("on");
    let mut epochs = registry.monitor_epochs();
    epochs.borrow_and_update();
    registry.note_js_blocking(Duration::from_millis(1));
    registry.open_load(OpenerId::PAGE, "app:///main.js", None);
    assert!(registry.mark_load(OpenerId::PAGE, LoadMark::MtsRenderStart));
    let mut binding = flush(OpenerId::PAGE, 0, &[], 1);
    binding.load = true;
    registry.bind_commit(binding);
    registry.note_presented(frame(1));
    assert!(epochs.has_changed().unwrap(), "the report task is woken");
    assert!(
        registry.js_blocking_anchor(epoch).is_some(),
        "the timer starts at the load"
    );
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
    // Another monitor's setter keeps this configuration current; turned on
    // again after the load, the timer is anchored at once.
    registry.set_long_task_threshold(None);
    assert!(registry.js_blocking_is_current(epoch));
    registry.set_js_blocking_monitor(None, ClockInstant::now());
    let again = registry
        .set_js_blocking_monitor(Some(blocking(0)), ClockInstant::now())
        .expect("on");
    assert!(again > epoch);
    assert!(!registry.js_blocking_is_current(epoch));
    assert!(registry.js_blocking_anchor(again).is_some());
}

#[test]
fn a_js_blocking_entry_writes_lynx_s_keys_and_types() {
    let origin = TimeOrigin::now();
    let report = JsBlockingEntry::from_totals(
        JsBlockingStage::Timer,
        Duration::from_millis(45),
        2,
        Duration::from_secs(5),
        Some(Duration::from_millis(5_001)),
    );
    let entry = PerformanceEntry::JsBlocking(report);
    assert_eq!(
        (entry.entry_type(), entry.name()),
        ("js_blocking", "js_blocking")
    );
    let keys: Vec<_> = entry
        .lynx_fields(&origin)
        .map(|(key, _)| key.into_owned())
        .collect();
    let expected = "entryType name stage total_blocking_time total_blocking_count \
                    total_duration blocking_ratio avg_blocking_time time_after_load_bundle";
    assert_eq!(keys.join(" "), expected);
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["stage"], "timer");
    assert_eq!(value["total_blocking_time"].as_i64(), Some(45));
    assert_eq!(value["total_blocking_count"].as_i64(), Some(2));
    assert_eq!(value["total_duration"].as_i64(), Some(5_000));
    assert_eq!(value["time_after_load_bundle"].as_i64(), Some(5_001));
    assert!((value["blocking_ratio"].as_f64().unwrap() - 0.009).abs() < 1e-12);
    assert!((value["avg_blocking_time"].as_f64().unwrap() - 22.5).abs() < 1e-9);
    // A `loadBundle` report has no `time_after_load_bundle`, and a ratio
    // over a zero span is `null` rather than invalid JSON.
    let load = PerformanceEntry::JsBlocking(JsBlockingEntry::from_totals(
        JsBlockingStage::LoadBundle,
        Duration::from_millis(45),
        2,
        Duration::ZERO,
        None,
    ));
    let mut json = String::new();
    load.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["stage"], "loadBundle");
    assert!(value["blocking_ratio"].is_null());
    assert!(value.get("time_after_load_bundle").is_none());
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
fn the_memory_monitor_is_off_by_default_and_reads_nothing() {
    assert!(EntryTypes::ALL.contains(EntryTypes::MEMORY));
    let config = MemoryConfig::default();
    assert_eq!(config.report_interval, Duration::from_mins(20));
    assert_eq!(config.acquisition_delay, Duration::from_secs(2));
    assert_eq!(config.change_threshold, 0);
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::ALL);
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 10)));
    let epoch = registry.set_memory_monitor(Some(memory(0))).expect("on");
    assert!(registry.memory_is_current(epoch));
    // Another monitor's setter leaves the memory monitor's epoch current.
    registry.set_long_task_threshold(None);
    assert!(registry.memory_is_current(epoch));
    assert_eq!(registry.set_memory_monitor(None), None);
    assert!(!registry.memory_is_current(epoch));
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 10)));
    assert!(memory_entries(&registry).is_empty());
    assert_eq!(registry.generation(), 0);
}

#[test]
fn memory_reports_follow_lynx_s_change_rule() {
    let registry = PipelineRegistry::detached();
    registry.observe_rust(EntryTypes::MEMORY);
    // A zero threshold reports every reading that changes a category.
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
        "the unchanged reading was dropped"
    );

    // A threshold: the first reading after turning the monitor on is
    // reported whatever it changed, and starts afresh.
    registry.set_memory_monitor(Some(memory(1_000)));
    assert!(registry.note_memory(reading(MemoryCategory::MainThreadScriptingEngine, 10)));
    // Kept, not reported: the total moved by less than the threshold.
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 900)));
    // This one takes the total 1 000 past the last report's.
    assert!(registry.note_memory(reading(
        MemoryCategory::BackgroundThreadScriptingEngine,
        100
    )));
    // A drop counts as much as a rise, measured from the last report.
    assert!(!registry.note_memory(reading(MemoryCategory::LynxTasmElement, 0)));
    assert!(registry.note_memory(reading(MemoryCategory::BackgroundThreadScriptingEngine, 0)));
    let entries = memory_entries(&registry);
    let totals: Vec<u64> = entries.iter().map(|entry| entry.size_bytes).collect();
    assert_eq!(totals, [10, 1_010, 10]);
    assert_eq!(entries[0].items.len(), 1, "the readings started afresh");
    assert_eq!(entries[1].items.len(), 3, "every category's latest reading");
}

#[test]
fn a_memory_entry_writes_lynx_s_nested_maps_and_a_mark_stays_one_member() {
    let origin = TimeOrigin::now();
    let entry = PerformanceEntry::Memory(MemoryEntry {
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
    });
    assert_eq!((entry.entry_type(), entry.name()), ("memory", "memory"));
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

    // A framework mark's own `.` and `\` nest nothing in the JSON, and the
    // BTS's record carries the mark's name as it was.
    let mut timing = pipeline(&origin);
    timing.framework_rendering_timing.marks = vec![
        (Arc::from("a.b"), at(&origin, 1)),
        (Arc::from("c\\d"), at(&origin, 2)),
    ];
    let entry = PerformanceEntry::Pipeline(Box::new(timing));
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert!(
        value["frameworkRenderingTiming"]["a.b"].is_number(),
        "{value}"
    );
    assert!(
        value["frameworkRenderingTiming"]["c\\d"].is_number(),
        "{value}"
    );
    assert!(value["frameworkRenderingTiming"].get("a").is_none());
    let record = encode_entries(&[entry], &origin);
    assert!(
        record.contains("28:frameworkRenderingTiming.a.b"),
        "{record}"
    );
    assert!(
        record.contains("28:frameworkRenderingTiming.c\\d"),
        "{record}"
    );
}

fn lazy_bundle(origin: &TimeOrigin, url: &str) -> LazyBundleEntry {
    LazyBundleEntry {
        component_url: Arc::from(url),
        size_bytes: None,
        load_success: true,
        require: at(origin, 1)..at(origin, 5),
        decode: None,
    }
}

fn bundle_urls(entries: Vec<PerformanceEntry>) -> Vec<String> {
    entries
        .into_iter()
        .map(|entry| match entry {
            PerformanceEntry::LazyBundle(bundle) => bundle.component_url.to_string(),
            other => panic!("a lazy bundle: {other:?}"),
        })
        .collect()
}

#[test]
fn a_lazy_bundle_is_timed_while_observed_and_reaches_rust_and_the_fetching_realm_s_bts() {
    let registry = PipelineRegistry::detached();
    let origin = registry.time_origin();
    assert!(EntryTypes::ALL.contains(EntryTypes::RESOURCE));
    assert!(!registry.times_resources(), "nobody observes at first");
    registry.observe_rust(EntryTypes::PIPELINE | EntryTypes::MEMORY);
    assert!(!registry.times_resources(), "other types time no fetch");
    // Nobody asks: dropped, and the embedder is not woken.
    assert!(!registry.note_lazy_bundle(
        Opener::Realm(OpenerId::PAGE),
        lazy_bundle(&origin, "app:///dropped.bundle")
    ));
    assert_eq!(registry.generation(), 0);

    let page_bts = WorkerKey::new(1);
    let card_bts = WorkerKey::new(2);
    let card = registry.allocate_opener();
    registry.bind_worker(page_bts, OpenerId::PAGE);
    registry.bind_worker(card_bts, card);
    registry.observe_worker(card_bts, Some(EntryTypes::NONE));
    registry.observe_worker(page_bts, Some(EntryTypes::RESOURCE));
    assert!(registry.times_resources(), "a BTS observer of resource");
    registry.observe_rust(EntryTypes::RESOURCE);
    // The page realm's fetch and its BTS's own reach that BTS; the card's
    // BTS does not ask for `resource`, so the card's reaches Rust alone.
    for (opener, url) in [
        (Opener::Realm(OpenerId::PAGE), "app:///page.bundle"),
        (Opener::Worker(page_bts), "app:///bts.bundle"),
        (Opener::Realm(card), "app:///card.bundle"),
    ] {
        assert!(registry.note_lazy_bundle(opener, lazy_bundle(&origin, url)));
    }
    assert_eq!(registry.take_wakes(), vec![page_bts]);
    assert_eq!(
        bundle_urls(registry.take_completed_for_rust()),
        [
            "app:///page.bundle",
            "app:///bts.bundle",
            "app:///card.bundle"
        ]
    );
    assert_eq!(
        bundle_urls(registry.take_completed_for_worker(page_bts)),
        ["app:///page.bundle", "app:///bts.bundle"]
    );
    assert!(registry.take_completed_for_worker(card_bts).is_empty());

    registry.observe_rust(EntryTypes::NONE);
    registry.forget_worker(page_bts);
    assert!(!registry.times_resources(), "the observers are gone");
}

#[test]
fn a_fetch_s_lazy_bundle_entry_takes_the_fetcher_s_readings_and_writes_lynx_s_keys() {
    let origin = TimeOrigin::now();
    let instant = |millis| origin.instant + Duration::from_millis(millis);
    let outcome = crate::resource::FetchOutcome {
        size_bytes: Some(2048),
        received: Some(instant(4)),
        installed: Some(instant(5)..instant(7)),
    };
    let url = || Arc::from("app:///lazy.bundle");
    let fetched = LazyBundleEntry::from_fetch(url(), instant(1), instant(9), Some(&outcome));
    assert!(fetched.load_success);
    assert_eq!(fetched.size_bytes, Some(2048));
    assert_eq!(fetched.require, at(&origin, 1)..at(&origin, 4));
    assert_eq!(fetched.decode, Some(at(&origin, 5)..at(&origin, 7)));
    // A fetcher that read nothing: the delivery ends the require span.
    let silent = crate::resource::FetchOutcome::default();
    let quiet = LazyBundleEntry::from_fetch(url(), instant(1), instant(9), Some(&silent));
    assert!(quiet.load_success);
    assert_eq!(quiet.require, at(&origin, 1)..at(&origin, 9));
    assert_eq!((quiet.size_bytes, quiet.decode), (None, None));
    // A failed fetch carries no outcome.
    let failed = LazyBundleEntry::from_fetch(url(), instant(1), instant(9), None);
    assert!(!failed.load_success);
    assert_eq!(failed.require, at(&origin, 1)..at(&origin, 9));

    let entry = PerformanceEntry::LazyBundle(fetched);
    assert_eq!(
        (entry.entry_type(), entry.name()),
        ("resource", "lazyBundle")
    );
    let keys = |entry: &PerformanceEntry| -> Vec<String> {
        let fields = entry.lynx_fields(&origin);
        fields.map(|(key, _)| key.into_owned()).collect()
    };
    assert_eq!(
        keys(&entry).join(" "),
        "entryType name componentUrl size loadSuccess mode decodeStart decodeEnd requireStart \
         requireEnd"
    );
    let mut json = String::new();
    entry.write_lynx_json(&origin, &mut json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("the entry is JSON");
    assert_eq!(value["size"], 2048);
    assert_eq!(value["loadSuccess"], true);
    assert_eq!(value["mode"], "normal");
    assert!(value.get("sync").is_none(), "native's writer leaves it out");
    let span = value["requireEnd"]
        .as_f64()
        .zip(value["requireStart"].as_f64());
    assert!(span.is_some_and(|(end, start)| (end - start - 3.0).abs() < 1e-6));
    // A failed fetch: no size, no decode, and the boolean in the BTS record.
    let failed = PerformanceEntry::LazyBundle(failed);
    assert_eq!(
        keys(&failed).join(" "),
        "entryType name componentUrl loadSuccess mode requireStart requireEnd"
    );
    let record = encode_entries(&[failed], &origin);
    assert!(record.contains("11:loadSuccess2:b0"), "{record}");
    assert!(record.contains("9:entryType9:sresource"), "{record}");
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
fn a_card_s_load_is_owed_as_loadmetrics_once_whoever_observes_and_the_page_s_never() {
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
    assert!(registry.take_frame_loads().is_empty(), "owed once");
    assert!(
        registry.take_completed_for_rust().is_empty(),
        "to nobody else"
    );

    // Sent again after `set_extra_timing`, a load is not owed again.
    registry.observe_rust(EntryTypes::LOAD_BUNDLE);
    assert!(registry.set_extra_timing(&ExtraTiming {
        open_time: Some(SystemTime::now()),
        ..ExtraTiming::default()
    }));
    assert!(registry.take_frame_loads().is_empty());
}
