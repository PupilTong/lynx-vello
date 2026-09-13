//! The shipped fetcher drives real startup through concrete source completions.

use std::sync::Arc;
use std::time::Duration;

use bobcat_core::resource::ResourceErrorKind;
use bobcat_core::{
    DrawTarget, EngineEvent, EventRequester, LynxGroup, LynxView, LynxViewError, Painter,
    PreparsedDeclaration, PreparsedRule, PreparsedStyleSheet, StyleThreads, ViewSources,
};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use bobcat_source::PageSource;

struct Wake(flume::Sender<()>);
impl EventRequester for Wake {
    fn request_event(&self) {
        let _ = self.0.send(());
    }
}

async fn setup() -> (LynxGroup, Resources, flume::Receiver<()>) {
    let (wake, receiver) = flume::unbounded();
    let group = LynxGroup::new(Arc::new(Wake(wake)), StyleThreads::Sequential)
        .await
        .unwrap();
    let resources = Resources::new(
        ResourcesConfig {
            base_url: Some("app:///".parse().unwrap()),
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || panic!("source completions must not use the image/host wakeup"),
    );
    (group, resources, receiver)
}

/// One view and the painter that captures it. A painter is per view — there
/// is at most one — so a test that captures two pages builds two.
async fn view(
    group: &LynxGroup,
    resources: &Resources,
    sources: ViewSources,
) -> (LynxView<ViewResources>, Painter) {
    let view = group
        .create_lynx_view(32.0, 24.0, 1.0, resources.builder(), sources)
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();
    (view, painter)
}

fn boot(
    view: &mut LynxView<ViewResources>,
    receiver: &flume::Receiver<()>,
) -> Result<(), LynxViewError> {
    loop {
        // The engine waits its own realm timers out, so the only reason to
        // stop waiting here is a wakeup — or the generous hang budget.
        receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("startup wakes the host");
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => return Ok(()),
                EngineEvent::StartupFailed(error) => return Err(error),
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn text_and_preparsed_sheets_keep_cascade_order_before_entry() {
    let (group, resources, receiver) = setup().await;
    resources
        .register(
            "app:///main.js",
            r"
        globalThis.renderPage = () => {
            const page = __CreatePage('page', 0);
            const view = __CreateView(0);
            __SetClasses(view, 'box');
            __AppendElement(page, view);
        };
    ",
            Some("text/javascript"),
        )
        .unwrap();
    resources
        .register(
            "app:///first.css",
            ".box { width: 32px; height: 24px; background: red; }",
            Some("text/css"),
        )
        .unwrap();
    resources
        .register_style_sheet(
            "app:///second.css",
            PreparsedStyleSheet {
                rules: vec![PreparsedRule::Style {
                    selectors: ".box".into(),
                    declarations: vec![PreparsedDeclaration {
                        property: "background-color".into(),
                        value: "blue".into(),
                        important: false,
                    }],
                }],
            },
        )
        .unwrap();
    let (mut view, mut painter) = view(
        &group,
        &resources,
        ViewSources {
            style_sheets: vec!["first.css".into(), "second.css".into()],
            ..ViewSources::new("main.js")
        },
    )
    .await;
    boot(&mut view, &receiver).unwrap();
    let screenshot = painter.capture().unwrap();
    let offset = (12 * screenshot.size.width as usize + 16) * 4;
    assert_eq!(&screenshot.pixels[offset..offset + 4], &[0, 0, 255, 255]);
}

#[tokio::test]
async fn source_utf8_errors_keep_the_resolved_url() {
    for stylesheet in [false, true] {
        let (group, resources, receiver) = setup().await;
        resources
            .register(
                "app:///invalid.bin",
                vec![0x00, 0xff],
                Some("application/octet-stream"),
            )
            .unwrap();
        let sources = if stylesheet {
            ViewSources {
                style_sheets: vec!["invalid.bin".into()],
                ..ViewSources::new("never-requested.js")
            }
        } else {
            ViewSources::new("invalid.bin")
        };
        let (mut view, _painter) = view(&group, &resources, sources).await;
        let error = boot(&mut view, &receiver).unwrap_err();
        match (stylesheet, error) {
            (true, LynxViewError::InvalidStyleSheetEncoding { url, .. })
            | (false, LynxViewError::InvalidScriptEncoding { url, .. }) => {
                assert_eq!(url, "app:///invalid.bin");
            }
            (_, error) => panic!("unexpected source failure: {error}"),
        }
        assert!(view.pump().is_empty(), "failure arrives once");
    }
}

#[tokio::test]
async fn missing_source_fails_without_blocking_sibling_startup() {
    let (group, resources, receiver) = setup().await;
    let (mut failed, _failed_painter) =
        view(&group, &resources, ViewSources::new("missing.js")).await;
    // `app:` is a plausible scheme that nothing registered and no transport
    // serves, so resolution is where the load stops.
    match boot(&mut failed, &receiver) {
        Err(LynxViewError::Resource(error)) => {
            assert_eq!(error.kind, ResourceErrorKind::UnsupportedScheme);
        }
        outcome => panic!("unexpected outcome for a missing source: {outcome:?}"),
    }
    resources
        .register("app:///main.js", "", Some("text/javascript"))
        .unwrap();
    let (mut sibling, mut sibling_painter) =
        view(&group, &resources, ViewSources::new("main.js")).await;
    boot(&mut sibling, &receiver).unwrap();
    sibling_painter.tick(true).unwrap();
    assert!(failed.pump().is_empty());
}

/// A browser embedder learns the base only from the entry response, so it
/// names one with `set_base_url` after the system is already built. A source
/// specifier resolves against that base and not the one the config carried.
#[tokio::test]
async fn a_base_named_after_construction_resolves_a_relative_source() {
    let (group, resources, receiver) = setup().await;
    resources.set_base_url(Some("app:///nested/".parse().unwrap()));
    assert_eq!(
        resources.base_url().map(|base| base.to_string()).as_deref(),
        Some("app:///nested/")
    );
    resources
        .register(
            "app:///nested/main.js",
            r"
        globalThis.renderPage = () => {
            const page = __CreatePage('page', 0);
            const view = __CreateView(0);
            __SetInlineStyles(view, 'width:32px;height:24px;background:blue');
            __AppendElement(page, view);
        };
    ",
            Some("text/javascript"),
        )
        .unwrap();
    let (mut view, mut painter) = view(&group, &resources, ViewSources::new("main.js")).await;
    boot(&mut view, &receiver).unwrap();
    let screenshot = painter.capture().unwrap();
    let offset = (12 * screenshot.size.width as usize + 16) * 4;
    assert_eq!(&screenshot.pixels[offset..offset + 4], &[0, 0, 255, 255]);
}

#[tokio::test]
async fn workers_load_relative_to_entry_and_route_back_to_their_own_views() {
    let (group, first_resources, receiver) = setup().await;
    let second_resources = Resources::new(
        ResourcesConfig {
            base_url: Some("app:///".parse().unwrap()),
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || {},
    );
    for (resources, color, entry) in [
        (&first_resources, "blue", "app:///nested/first.js"),
        (&second_resources, "red", "app:///nested/second.js"),
    ] {
        resources.register(entry, r"
            import { Worker } from 'bobcat-internal';
            const page = __CreatePage();
            const box = __CreateView();
            __SetInlineStyles(box, 'width:32px;height:24px;background:black');
            __AppendElement(page, box);
            const received = [];
            // Both requests can reach the painter in the same pump turn.
            for (const name of ['first', 'second']) {
                const worker = new Worker('./worker.js', {name});
                worker.onmessage = event => {
                    received.push(event.data);
                    if (received.length === 2) {
                        if (received[0].color !== received[1].color ||
                            received[0].name === received[1].name) throw Error('wrong worker context');
                        __SetInlineStyles(box, `width:32px;height:24px;background:${event.data.color}`);
                    }
                    worker.terminate();
                };
                worker.postMessage('ready');
            }
        ", Some("text/javascript")).unwrap();
        resources
            .register(
                "app:///nested/worker.js",
                format!("onmessage = () => postMessage({{name, color: '{color}'}});"),
                Some("text/javascript"),
            )
            .unwrap();
    }
    let (mut first, mut first_painter) = view(
        &group,
        &first_resources,
        ViewSources::new("nested/first.js"),
    )
    .await;
    let (mut second, mut second_painter) = view(
        &group,
        &second_resources,
        ViewSources::new("nested/second.js"),
    )
    .await;
    boot(&mut first, &receiver).unwrap();
    boot(&mut second, &receiver).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let mut ready = true;
        for (view, painter, color) in [
            (&mut first, &mut first_painter, [0, 0, 255, 255]),
            (&mut second, &mut second_painter, [255, 0, 0, 255]),
        ] {
            for event in view.pump() {
                match event {
                    EngineEvent::StartupFailed(error) => panic!("startup: {error}"),
                    EngineEvent::WorkerFailed(error)
                    | EngineEvent::ListenerFailed(error)
                    | EngineEvent::ScriptRunError(error) => panic!("script: {error}"),
                    _ => {}
                }
            }
            let screenshot = painter.capture().unwrap();
            let offset = (12 * screenshot.size.width as usize + 16) * 4;
            ready &= screenshot.pixels[offset..offset + 4] == color;
        }
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker messages did not paint"
        );
        let _ = receiver.recv_timeout(Duration::from_millis(5));
    }
}

#[tokio::test]
async fn xml_background_loads_esm_through_the_view_fetcher() {
    let (group, resources, receiver) = setup().await;
    resources
        .register("app:///dep.js", "export const value = 42;", None)
        .unwrap();
    let page = PageSource::from_bytes(
        &"app:///card.lynx.xml".parse().unwrap(),
        br#"
        <lynx engine-version="4.2">
          <script thread="main"><![CDATA[
            __CreatePage();
            lynx.getJSContext().dispatchEvent({type: 'initialize', data: null});
          ]]></script>
          <script thread="background"><![CDATA[
            import {lynx} from 'bobcat:bts-runtime';
            const {value} = await import('app:///dep.js');
            await new Promise(resolve => setTimeout(resolve, 1));
            if (value !== 42) throw Error('source value');
            lynx.getCoreContext().addEventListener('initialize', () => {
                lynx.reportError('BTS sources ready');
            });
          ]]></script>
        </lynx>
        "#,
    )
    .unwrap();
    page.register_with(&resources);
    let mut view = group
        .create_lynx_view(32.0, 24.0, 1.0, resources.builder(), page.view_sources())
        .unwrap();
    assert!(!view.is_ready());
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut script_finished = false;
    let mut received_queued_event = false;
    while !script_finished || !received_queued_event {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => script_finished = true,
                EngineEvent::ScriptReported { message, .. } => {
                    assert_eq!(message, "BTS sources ready");
                    received_queued_event = true;
                }
                EngineEvent::StartupFailed(error) => panic!("startup: {error}"),
                EngineEvent::WorkerFailed(error)
                | EngineEvent::ListenerFailed(error)
                | EngineEvent::ScriptRunError(error) => panic!("script: {error}"),
                _ => {}
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "BTS did not finish loading"
        );
        let _ = receiver.recv_timeout(Duration::from_millis(5));
    }
    assert!(view.is_ready());
}

#[tokio::test]
async fn dynamic_import_loads_relative_static_dependencies_and_waits_for_top_level_await() {
    let (group, resources, receiver) = setup().await;
    for (url, source) in [
        (
            "app:///page/main.js",
            r"
            const path = './chunks/unused/../answer.js';
            const [first, second] = await Promise.all([import(path), import('./chunks/answer.js')]);
            if (first !== second || first.answer !== 42 || globalThis.moduleRuns !== 1)
                throw Error('module identity, relative resolution or top-level await failed');
            globalThis.renderPage = () => {
                const page = __CreatePage('page', 0);
                const view = __CreateView(0);
                __SetInlineStyles(view, 'width:32px;height:24px;background:blue');
                __AppendElement(page, view);
            };
        ",
        ),
        (
            "app:///page/chunks/answer.js",
            r"
            import { value } from '../value.js';
            globalThis.moduleRuns = (globalThis.moduleRuns ?? 0) + 1;
            await new Promise(resolve => setTimeout(resolve, 1));
            export const answer = value + (await import('./one.js')).value;
        ",
        ),
        ("app:///page/value.js", "export const value = 41;"),
        ("app:///page/chunks/one.js", "export const value = 1;"),
    ] {
        resources
            .register(url, source, Some("text/javascript"))
            .unwrap();
    }
    let (mut view, mut painter) = view(&group, &resources, ViewSources::new("page/main.js")).await;
    boot(&mut view, &receiver).unwrap();
    let screenshot = painter.capture().unwrap();
    let offset = (12 * screenshot.size.width as usize + 16) * 4;
    assert_eq!(&screenshot.pixels[offset..offset + 4], &[0, 0, 255, 255]);
}

#[tokio::test]
async fn import_failures_reject_promises_and_only_uncaught_startup_failures_end_boot() {
    for caught in [true, false] {
        let (group, resources, receiver) = setup().await;
        let source = if caught {
            r"
                let rejected = 0;
                for (let i = 0; i < 2; i++) {
                    try { await import('./missing.js'); }
                    catch (error) { if (!(error instanceof TypeError)) throw error; rejected++; }
                }
                try { await import('./broken.js'); }
                catch (error) { if (!(error instanceof SyntaxError)) throw error; rejected++; }
                try { await import('bare-name'); }
                catch (error) { if (!(error instanceof TypeError)) throw error; rejected++; }
                if (rejected !== 4) throw Error('missing import rejection');
            "
        } else {
            "await import('./missing.js');"
        };
        resources
            .register("app:///main.js", source, Some("text/javascript"))
            .unwrap();
        resources
            .register(
                "app:///broken.js",
                "export const = ;",
                Some("text/javascript"),
            )
            .unwrap();
        let (mut view, _painter) = view(&group, &resources, ViewSources::new("main.js")).await;
        let outcome = boot(&mut view, &receiver);
        if caught {
            outcome.unwrap();
        } else {
            let error = outcome.unwrap_err();
            assert!(error.to_string().contains("missing.js"), "{error:?}");
        }
    }
}

#[tokio::test]
async fn sibling_views_can_import_the_same_urls_with_independent_module_instances() {
    let (group, resources, receiver) = setup().await;
    resources
        .register(
            "app:///main.js",
            r"
        const module = await import('./shared.js');
        if (module.value !== 42 || globalThis.runs !== 1) throw Error('realm isolation');
    ",
            Some("text/javascript"),
        )
        .unwrap();
    resources
        .register(
            "app:///shared.js",
            r"
        globalThis.runs = (globalThis.runs ?? 0) + 1;
        export const value = 42;
    ",
            Some("text/javascript"),
        )
        .unwrap();
    let (mut first, _first_painter) = view(&group, &resources, ViewSources::new("main.js")).await;
    let (mut second, _second_painter) = view(&group, &resources, ViewSources::new("main.js")).await;
    let mut finished = 0;
    while finished < 2 {
        receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("imports wake the host");
        for event in first.pump().into_iter().chain(second.pump()) {
            match event {
                EngineEvent::ScriptFinished => finished += 1,
                EngineEvent::StartupFailed(error) => panic!("sibling failed: {error}"),
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn imports_started_after_boot_can_commit_a_later_frame() {
    let (group, resources, receiver) = setup().await;
    resources
        .register(
            "app:///main.js",
            r"
        globalThis.renderPage = () => {
            const page = __CreatePage('page', 0);
            const child = __CreateView(0);
            __SetInlineStyles(child, 'width:32px;height:24px;background:red');
            __AppendElement(page, child);
            setTimeout(async () => {
                const module = await import('./color.js');
                __SetInlineStyles(child, `width:32px;height:24px;background:${module.color}`);
                __FlushElementTree();
            }, 25);
        };
    ",
            Some("text/javascript"),
        )
        .unwrap();
    resources
        .register(
            "app:///color.js",
            "export const color = 'blue';",
            Some("text/javascript"),
        )
        .unwrap();
    let (mut view, mut painter) = view(&group, &resources, ViewSources::new("main.js")).await;
    boot(&mut view, &receiver).unwrap();
    let stop = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let screenshot = painter.capture().unwrap();
        let offset = (12 * screenshot.size.width as usize + 16) * 4;
        if screenshot.pixels[offset..offset + 4] == [0, 0, 255, 255] {
            break;
        }
        assert!(
            std::time::Instant::now() < stop,
            "import never committed its frame"
        );
        // The engine waits its own timers out; this is a poll interval for a
        // wakeup that has no deadline of its own to name.
        let _ = receiver.recv_timeout(Duration::from_millis(100));
        for event in view.pump() {
            assert!(
                !matches!(
                    event,
                    EngineEvent::StartupFailed(_)
                        | EngineEvent::ScriptRunError(_)
                        | EngineEvent::TimerFailed(_)
                ),
                "{event:?}"
            );
        }
    }
}
