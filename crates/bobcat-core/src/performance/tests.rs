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
    registry.observe_worker(worker, true);
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
