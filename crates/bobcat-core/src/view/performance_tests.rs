//! Performance entries end to end: a real view, its BTS worker and an
//! offscreen painter, so a pipeline is bound by a real flush, completed by
//! a real present, and taken by `pump` and by the BTS's own observer.

use std::time::{Duration, Instant, SystemTime};

use super::*;
use crate::performance::{PipelineOrigin, PipelineTiming, Timestamp};
use crate::test_support::{TestEngine, TestViewSpec};

/// A card whose first screen carries the flag `first`, whose `updatePage`
/// and `rLynxChange` re-render it, the second as `ReactLynx`'s BTS update does:
/// with the BTS's `pipelineOptions` handed to the flush.
const CARD: &str = r"
    function paint(width) {
      __SetInlineStyles(view, `width:${width}px;height:10px;background-color:red`);
    }
    globalThis.renderPage = function () {
      const page = __CreatePage('card', 0);
      globalThis.view = __CreateView(0);
      paint(10);
      __SetAttribute(view, '__lynx_timing_flag', 'first');
      __AppendElement(page, view);
    };
    globalThis.updatePage = function () {
      paint(14);
      __FlushElementTree();
    };
    globalThis.rLynxChange = function (data) {
      lynx.performance._markTiming(data.pipelineOptions.pipelineID, 'mtsRenderStart');
      paint(18);
      lynx.performance._markTiming(data.pipelineOptions.pipelineID, 'mtsRenderEnd');
      __FlushElementTree(view, { pipelineOptions: data.pipelineOptions });
    };
";

/// A BTS that logs every pipeline entry it observes, and once the load is
/// reported starts one `ReactLynx`-shaped update: a pipeline that carries
/// timestamps, a framework mark, a flag bound to it, and the call that hands
/// its options to the main thread's flush.
const OBSERVING_BTS: &str = r"
    import { lynx, console } from 'bobcat:bts-runtime';
    let updated = false;
    lynx.performance.createObserver(entry => {
      console.log('perf ' + JSON.stringify(entry));
      if (entry.name !== 'loadBundle' || updated) return;
      updated = true;
      const options = lynx.performance._generatePipelineOptions();
      options.needTimestamps = true;
      options.pipelineOrigin = 'updateTriggeredByBts';
      options.dsl = 'reactLynx';
      options.stage = 'update';
      lynx.performance._onPipelineStart(options.pipelineID, options);
      lynx.performance._markTiming(options.pipelineID, 'diffVdomStart');
      lynx.performance._bindPipelineIdWithTimingFlag(options.pipelineID, 'bts-flag');
      lynx.getNativeApp().callLepusMethod('rLynxChange', { pipelineOptions: options });
    }).observe(['pipeline']);
";

fn engine(background: Option<&str>) -> TestEngine {
    let mut spec = TestViewSpec::new(CARD).offscreen(32.0, 24.0);
    if let Some(background) = background {
        spec = spec.with_background_entry(background);
    }
    spec.create(Arc::new(NoWakeup))
}

/// One host turn: an offscreen frame drawn, then the view's own turn, whose
/// events are kept. Any failure fails the test with what it said.
fn turn(engine: &mut TestEngine, events: &mut Vec<EngineEvent>) {
    let _ = engine.tick(true);
    for event in engine.pump() {
        match event {
            EngineEvent::StartupFailed(_)
            | EngineEvent::Panicked(_)
            | EngineEvent::ScriptRunError(_)
            | EngineEvent::ListenerFailed(_)
            | EngineEvent::WorkerThrew { .. }
            | EngineEvent::WorkerEnded { .. } => panic!("a realm failed: {event:?}"),
            event => events.push(event),
        }
    }
}

/// Turns until `done` holds of what the turns reported.
fn drive_until(
    engine: &mut TestEngine,
    events: &mut Vec<EngineEvent>,
    mut done: impl FnMut(&[EngineEvent]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(events) {
        assert!(Instant::now() < deadline, "never came: {events:?}");
        turn(engine, events);
        std::thread::yield_now();
    }
}

fn performance(events: &[EngineEvent]) -> impl Iterator<Item = &PerformanceEntry> {
    events.iter().filter_map(|event| match event {
        EngineEvent::Performance(entry) => Some(entry),
        _ => None,
    })
}

fn load_entries(events: &[EngineEvent]) -> Vec<&crate::performance::LoadBundleTiming> {
    performance(events)
        .filter_map(|entry| match entry {
            PerformanceEntry::LoadBundle(load) => Some(&**load),
            _ => None,
        })
        .collect()
}

fn pipeline_named<'a>(events: &'a [EngineEvent], flag: &str) -> Option<&'a PipelineTiming> {
    performance(events).find_map(|entry| match entry {
        PerformanceEntry::Pipeline(pipeline) if pipeline.identifier.as_deref() == Some(flag) => {
            Some(&**pipeline)
        }
        _ => None,
    })
}

/// Every stage of a pipeline in order, from its start to its present.
fn assert_ordered(pipeline: &PipelineTiming) {
    let stages: [Timestamp; 10] = [
        pipeline.pipeline_start,
        pipeline.resolve.start,
        pipeline.resolve.end,
        pipeline.layout.start,
        pipeline.layout.end,
        pipeline.painting_ui_operation_execute.start,
        pipeline.painting_ui_operation_execute.end,
        pipeline.layout_ui_operation_execute.start,
        pipeline.layout_ui_operation_execute.end,
        pipeline.paint_end,
    ];
    assert!(
        stages.windows(2).all(|pair| pair[0] <= pair[1]),
        "stages out of order: {pipeline:?}"
    );
    assert_eq!(pipeline.paint_end, pipeline.pipeline_end);
}

#[test]
fn a_flagged_first_screen_reports_its_load_to_rust_and_to_the_bts() {
    let mut engine = engine(Some(OBSERVING_BTS));
    engine.view.observe_performance(EntryTypes::ALL);
    let mut events = Vec::new();
    let perf_logged = |events: &[EngineEvent]| {
        events.iter().any(|event| {
            matches!(event, EngineEvent::ConsoleMessage { message, .. }
                if message.starts_with("perf ") && message.contains(r#""identifier":"first""#))
        })
    };
    drive_until(&mut engine, &mut events, |events| {
        !load_entries(events).is_empty()
            && pipeline_named(events, "first").is_some()
            && perf_logged(events)
    });

    let [load] = load_entries(&events)[..] else {
        panic!("one load entry: {events:?}");
    };
    assert_eq!(load.pipeline.origin, PipelineOrigin::LoadBundle);
    assert_eq!(load.pipeline.identifier, None);
    assert_eq!(&*load.url, "app:///main.js");
    let vm_execute = load.vm_execute.clone().expect("the entry's evaluation");
    let mts_render = load.pipeline.mts_render.clone().expect("the first render");
    let load_core = load.load_core.clone().expect("the BTS opened");
    let load_background = load.load_background.clone().expect("the BTS entry loaded");
    for range in [
        &load.create_lynx,
        &load.load_bundle,
        &vm_execute,
        &mts_render,
        &load_core,
        &load_background,
    ] {
        assert!(range.start <= range.end, "{range:?}");
    }
    assert!(load.create_lynx.end <= load.load_bundle.start);
    assert!(load.load_bundle.start <= mts_render.start);
    assert!(load.lynx_fcp.duration() > Duration::ZERO);
    assert!(load.fcp.is_none() && load.total_fcp.is_none());
    assert_ordered(&load.pipeline);

    let first = pipeline_named(&events, "first").expect("the flag's entry");
    assert_eq!(first.origin, PipelineOrigin::LoadBundle);
    assert_ordered(first);

    // The BTS's observer received the same pipeline, as Lynx's JSON.
    let message = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. }
                if message.contains(r#""identifier":"first""#) =>
            {
                message.strip_prefix("perf ")
            }
            _ => None,
        })
        .expect("the BTS logged the flag's entry");
    let entry: serde_json::Value = serde_json::from_str(message).expect("an entry is JSON");
    assert_eq!(entry["entryType"], "pipeline");
    assert_eq!(entry["name"], "loadBundle");
    assert_eq!(entry["identifier"], "first");
    assert_eq!(entry["hostPlatformTiming"]["hostPlatformType"], "vello");
    let paint_end = entry["paintEnd"].as_f64().expect("a numeric paintEnd");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs_f64()
        * 1000.0;
    assert!((now - paint_end).abs() < 60_000.0, "{paint_end} vs {now}");
}

#[test]
fn a_native_update_with_a_flag_reports_its_pipeline() {
    let mut engine = engine(None);
    engine.view.observe_performance(EntryTypes::PIPELINE);
    let mut events = Vec::new();
    drive_until(&mut engine, &mut events, |events| {
        pipeline_named(events, "first").is_some()
    });
    engine
        .view
        .update_data(r#"{"__lynx_timing_flag":"n1"}"#.to_owned(), String::new())
        .expect("the view is ready");
    drive_until(&mut engine, &mut events, |events| {
        pipeline_named(events, "n1").is_some()
    });
    let update = pipeline_named(&events, "n1").expect("the update's entry");
    assert_eq!(update.origin, PipelineOrigin::UpdateTriggeredByNative);
    assert_ordered(update);
    assert!(
        load_entries(&events).is_empty(),
        "only pipeline entries were asked for"
    );
}

#[test]
fn a_bts_update_reports_its_pipeline_with_the_framework_s_marks() {
    let mut engine = engine(Some(OBSERVING_BTS));
    engine.view.observe_performance(EntryTypes::PIPELINE);
    let mut events = Vec::new();
    drive_until(&mut engine, &mut events, |events| {
        pipeline_named(events, "bts-flag").is_some()
    });
    let update = pipeline_named(&events, "bts-flag").expect("the update's entry");
    assert_eq!(update.origin, PipelineOrigin::UpdateTriggeredByBts);
    let framework = &update.framework_rendering_timing;
    assert_eq!(framework.dsl.as_deref(), Some("reactLynx"));
    assert_eq!(framework.stage.as_deref(), Some("update"));
    assert!(
        framework
            .marks
            .iter()
            .any(|(key, at)| &**key == "diffVdomStart" && *at <= update.resolve.start),
        "{framework:?}"
    );
    let render = update.mts_render.clone().expect("the main thread's render");
    assert!(update.pipeline_start <= render.start && render.end <= update.resolve.start);
    assert_ordered(update);
}

#[test]
fn extra_timing_after_the_load_reports_the_load_again_once() {
    let mut engine = engine(None);
    engine.view.observe_performance(EntryTypes::LOAD_BUNDLE);
    let mut events = Vec::new();
    drive_until(&mut engine, &mut events, |events| {
        !load_entries(events).is_empty()
    });
    // 50 ms before the view's own origin: before its load began.
    let origin = engine.view.time_origin();
    let opened = SystemTime::UNIX_EPOCH + Duration::from_secs_f64(origin.unix_millis() / 1000.0)
        - Duration::from_millis(50);
    let extra = crate::performance::ExtraTiming {
        open_time: Some(opened),
        container_init_start: Some(opened),
        container_init_end: Some(opened),
        prepare_template_start: Some(opened),
        prepare_template_end: Some(opened),
    };
    engine.view.set_extra_timing(extra).expect("a live view");
    drive_until(&mut engine, &mut events, |events| {
        load_entries(events).len() == 2
    });
    let again = load_entries(&events)[1];
    assert_eq!(again.extra, extra);
    let fcp = again.fcp.expect("prepareTemplateStart was set");
    let total = again.total_fcp.expect("openTime was set");
    assert!(fcp.duration() >= again.lynx_fcp.duration());
    assert!(total.duration() >= Duration::from_millis(50));
    // A key set twice is set once.
    engine.view.set_extra_timing(extra).expect("a live view");
    for _ in 0..5 {
        turn(&mut engine, &mut events);
    }
    assert_eq!(load_entries(&events).len(), 2);
}

#[test]
fn a_view_nobody_observes_reports_nothing_and_keeps_no_record() {
    let mut engine = engine(None);
    let registry = Arc::clone(&engine.view.seat().registry);
    let mut events = Vec::new();
    // The load completes — and is freed — once its first frame is presented
    // and its BTS has loaded its (empty) entry.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        turn(&mut engine, &mut events);
        let ready = events
            .iter()
            .any(|event| matches!(event, EngineEvent::ScriptFinished));
        if ready && registry.records_for_test().is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{:?}",
            registry.records_for_test()
        );
        std::thread::yield_now();
    }
    for _ in 0..3 {
        turn(&mut engine, &mut events);
    }
    assert!(performance(&events).next().is_none(), "{events:?}");
    assert!(registry.records_for_test().is_empty());
}
