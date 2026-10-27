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

/// Solid em squares, so a text run is ink whatever the machine's fonts.
const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

/// A card whose first screen is one coloured box — a first paint, and no
/// contentful one — and whose `updatePage` adds a text: the first
/// contentful paint.
const TEXT_CARD: &str = r"
    globalThis.renderPage = function () {
      globalThis.page = __CreatePage('card', 0);
      const view = __CreateView(0);
      __SetInlineStyles(view, 'width:10px;height:10px;background-color:red');
      __AppendElement(page, view);
    };
    globalThis.updatePage = function () {
      const text = __CreateText(0);
      __AppendElement(text, __CreateRawText('Hello'));
      __AppendElement(page, text);
      __FlushElementTree();
    };
";

/// A BTS that logs every `paint` entry it observes, having said it
/// observes.
const PAINT_BTS: &str = r"
    import { lynx, console } from 'bobcat:bts-runtime';
    lynx.performance.createObserver(entry => {
      console.log('paint ' + JSON.stringify(entry));
    }).observe(['paint']);
    console.log('observing paint');
";

fn text_engine(background: Option<&str>) -> TestEngine {
    let mut spec = TestViewSpec::new(TEXT_CARD)
        .offscreen(64.0, 48.0)
        .with_default_font("Ahem", AHEM);
    if let Some(background) = background {
        spec = spec.with_background_entry(background);
    }
    spec.create(Arc::new(NoWakeup))
}

fn paint_entries(events: &[EngineEvent]) -> Vec<crate::performance::PaintEntry> {
    performance(events)
        .filter_map(|entry| match entry {
            PerformanceEntry::Paint(paint) => Some(*paint),
            _ => None,
        })
        .collect()
}

#[test]
fn observing_paint_reports_first_paint_then_first_contentful_paint_once_each() {
    use crate::performance::PaintName;

    let mut engine = text_engine(None);
    let registry = Arc::clone(&engine.view.seat().registry);
    engine.view.observe_performance(EntryTypes::PAINT);
    assert!(registry.wants_content_detection());
    let mut events = Vec::new();
    drive_until(&mut engine, &mut events, |events| {
        !paint_entries(events).is_empty()
    });
    // The first screen is a coloured box: a first paint, nothing contentful.
    for _ in 0..3 {
        turn(&mut engine, &mut events);
    }
    let names = |events: &[EngineEvent]| -> Vec<PaintName> {
        paint_entries(events)
            .into_iter()
            .map(|paint| paint.name)
            .collect()
    };
    assert_eq!(names(&events), [PaintName::FirstPaint]);
    assert!(
        registry.wants_content_detection(),
        "still waiting for content"
    );

    engine
        .view
        .update_data("{}".to_owned(), String::new())
        .expect("the view is ready");
    drive_until(&mut engine, &mut events, |events| {
        paint_entries(events).len() == 2
    });
    for _ in 0..3 {
        turn(&mut engine, &mut events);
    }
    assert_eq!(
        names(&events),
        [PaintName::FirstPaint, PaintName::FirstContentfulPaint]
    );
    let [first, contentful] = paint_entries(&events)[..] else {
        unreachable!("two entries, asserted above");
    };
    assert!(first.start < contentful.start);
    assert!(
        !registry.wants_content_detection(),
        "detection ends with the contentful paint"
    );
    assert!(
        performance(&events).all(|entry| matches!(entry, PerformanceEntry::Paint(_))),
        "only paint entries were asked for"
    );
}

#[test]
fn observing_loads_alone_reports_no_paint_and_detects_no_content() {
    let mut engine = text_engine(None);
    let registry = Arc::clone(&engine.view.seat().registry);
    engine.view.observe_performance(EntryTypes::LOAD_BUNDLE);
    let mut events = Vec::new();
    drive_until(&mut engine, &mut events, |events| {
        assert!(!registry.wants_content_detection());
        !load_entries(events).is_empty()
    });
    engine
        .view
        .update_data("{}".to_owned(), String::new())
        .expect("the view is ready");
    for _ in 0..5 {
        turn(&mut engine, &mut events);
        assert!(!registry.wants_content_detection());
    }
    assert!(paint_entries(&events).is_empty(), "{events:?}");
    assert_eq!(load_entries(&events).len(), 1);
}

#[test]
fn a_bts_paint_observer_receives_both_paint_entries() {
    let mut engine = text_engine(Some(PAINT_BTS));
    let mut events = Vec::new();
    let logged = |events: &[EngineEvent], text: &str| -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::ConsoleMessage { message, .. } if message.starts_with(text) => {
                    Some(message.clone())
                }
                _ => None,
            })
            .collect()
    };
    // The view's own turns alone until the BTS observes, so the first frame
    // is presented — and its first paint recorded — only once it does.
    let deadline = Instant::now() + Duration::from_secs(30);
    while logged(&events, "observing paint").is_empty() {
        assert!(
            Instant::now() < deadline,
            "the BTS never observed: {events:?}"
        );
        events.extend(engine.view.pump());
        std::thread::yield_now();
    }
    drive_until(&mut engine, &mut events, |events| {
        !logged(events, "paint {").is_empty()
    });
    engine
        .view
        .update_data("{}".to_owned(), String::new())
        .expect("the view is ready");
    drive_until(&mut engine, &mut events, |events| {
        logged(events, "paint {").len() == 2
    });
    for _ in 0..3 {
        turn(&mut engine, &mut events);
    }
    let entries: Vec<serde_json::Value> = logged(&events, "paint {")
        .iter()
        .map(|message| {
            serde_json::from_str(message.strip_prefix("paint ").expect("logged so"))
                .expect("an entry is JSON")
        })
        .collect();
    let [first, contentful] = &entries[..] else {
        panic!("two paint entries: {entries:?}");
    };
    for (entry, name) in [
        (first, "first-paint"),
        (contentful, "first-contentful-paint"),
    ] {
        let object = entry.as_object().expect("an object");
        let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["entryType", "name", "startTime"], "{entry}");
        assert_eq!(entry["entryType"], "paint");
        assert_eq!(entry["name"], name);
    }
    let first_at = first["startTime"].as_f64().expect("a number");
    let contentful_at = contentful["startTime"].as_f64().expect("a number");
    assert!(first_at < contentful_at, "{first_at} vs {contentful_at}");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs_f64()
        * 1000.0;
    assert!((now - first_at).abs() < 60_000.0, "{first_at} vs {now}");
    // The Rust embedder asked for nothing.
    assert!(performance(&events).next().is_none(), "{events:?}");
}

#[test]
fn an_unobserved_view_stops_timing_its_presents_once_its_load_completed() {
    let mut engine = engine(None);
    let registry = Arc::clone(&engine.view.seat().registry);
    assert!(
        registry.wants_presentation_timing(),
        "the view's first paint is not recorded yet, and its load waits"
    );
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        turn(&mut engine, &mut events);
        let ready = events
            .iter()
            .any(|event| matches!(event, EngineEvent::ScriptFinished));
        if ready && registry.records_for_test().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!registry.wants_presentation_timing());
    assert!(!registry.wants_content_detection());
    for _ in 0..3 {
        turn(&mut engine, &mut events);
        assert!(!registry.wants_presentation_timing());
    }
}

/// A BTS entry whose evaluation holds the worker thread for 20 ms: one job
/// of `bobcat-workers` that runs long.
const SPINNING_BTS: &str = r"
    const until = Date.now() + 20;
    while (Date.now() < until) {}
";

fn long_tasks(events: &[EngineEvent]) -> Vec<crate::performance::LongTaskEntry> {
    performance(events)
        .filter_map(|entry| match entry {
            PerformanceEntry::LongTask(task) => Some(*task),
            _ => None,
        })
        .collect()
}

#[test]
fn a_bts_job_that_runs_long_is_a_long_task_on_the_workers_thread() {
    use crate::performance::{JobKind, JsThreadName, LongTaskConfig};

    let mut engine = engine(Some(SPINNING_BTS));
    // Before the first turn, which is what serves the BTS entry's request.
    engine.view.set_long_task_monitor(Some(LongTaskConfig {
        threshold: Duration::from_millis(15),
    }));
    engine
        .view
        .observe_performance(EntryTypes::LONG_TASK | EntryTypes::LOAD_BUNDLE);
    let mut events = Vec::new();
    // The BTS entry is evaluated in the job that completes the module its
    // import asked for. Another job may run long too in a debug build, the
    // boot jobs most of all, so the spinning one is picked by its kind.
    let spinning = |task: &crate::performance::LongTaskEntry| {
        task.thread == JsThreadName::Workers && task.kind == JobKind::Module
    };
    drive_until(&mut engine, &mut events, |events| {
        long_tasks(events).iter().any(spinning) && !load_entries(events).is_empty()
    });
    let task = long_tasks(&events)
        .into_iter()
        .find(spinning)
        .expect("the spinning job");
    // At least the threshold: `Date.now()` reads whole milliseconds, so
    // the 20 ms spin itself can come out a little short.
    assert!(task.duration() >= Duration::from_millis(15), "{task:?}");
}

/// A BTS whose entry waits for a native module's answer before it finishes
/// loading — an answer the view sends it as one message, after the
/// embedder's first turn — and then keeps one answer in flight every 5 ms.
const WAITING_BTS: &str = r"
    import { NativeModules } from 'bobcat:bts-runtime';
    await new Promise(resolve => NativeModules.Echo.echo('first', resolve));
    setInterval(() => NativeModules.Echo.echo('tick', () => {}), 5);
";

fn blocking_reports(events: &[EngineEvent]) -> Vec<crate::performance::JsBlockingEntry> {
    performance(events)
        .filter_map(|entry| match entry {
            PerformanceEntry::JsBlocking(report) => Some(*report),
            _ => None,
        })
        .collect()
}

#[test]
fn the_js_blocking_monitor_reports_at_the_load_and_then_on_its_timer() {
    use crate::performance::{JsBlockingConfig, JsBlockingStage};

    let interval = Duration::from_millis(100);
    let mut engine = TestViewSpec::new(CARD)
        .offscreen(32.0, 24.0)
        .with_background_entry(WAITING_BTS)
        .with_echo_module("Echo")
        .create(Arc::new(NoWakeup));
    // Before the first turn, which is what invokes the module and sends
    // its answer to the BTS.
    engine
        .view
        .set_js_blocking_monitor(Some(JsBlockingConfig {
            threshold: Duration::ZERO,
            report_interval: interval,
        }))
        .expect("a live view");
    engine
        .view
        .observe_performance(EntryTypes::JS_BLOCKING | EntryTypes::LOAD_BUNDLE);
    let mut events = Vec::new();
    drive_until(&mut engine, &mut events, |events| {
        blocking_reports(events)
            .iter()
            .any(|report| report.stage == JsBlockingStage::Timer)
    });
    let reports = blocking_reports(&events);
    let load = reports.first().expect("the first report");
    assert_eq!(load.stage, JsBlockingStage::LoadBundle, "{reports:?}");
    assert!(
        load.total_blocking_count >= 1,
        "the module's answer waited: {load:?}"
    );
    assert_eq!(load.time_after_load_bundle, None);
    assert_eq!(load_entries(&events).len(), 1);
    let timer = reports
        .iter()
        .find(|report| report.stage == JsBlockingStage::Timer)
        .expect("a timer report");
    assert!(timer.total_blocking_count >= 1, "{timer:?}");
    let after = timer.time_after_load_bundle.expect("a timer report's");
    assert!(after >= interval, "{timer:?}");
    // Off: the task ends, and nothing more is stamped.
    engine
        .view
        .set_js_blocking_monitor(None)
        .expect("a live view");
    assert_eq!(engine.view.seat().registry.js_blocking_stamp(), None);
}
