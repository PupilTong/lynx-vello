//! The shipped fetcher drives real startup through concrete source completions.

use std::sync::Arc;
use std::time::Duration;

use bobcat_core::{
    DrawTarget, EngineEvent, EventRequester, LynxGroup, LynxView, LynxViewError,
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

async fn view(
    group: &LynxGroup,
    resources: &Resources,
    sources: ViewSources,
) -> LynxView<ViewResources> {
    group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            DrawTarget::Offscreen,
            resources.builder(),
            sources,
        )
        .await
        .unwrap()
}

fn boot(
    view: &mut LynxView<ViewResources>,
    receiver: &flume::Receiver<()>,
) -> Result<(), LynxViewError> {
    loop {
        receiver
            .recv_timeout(Duration::from_mins(2))
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
    let mut view = view(
        &group,
        &resources,
        ViewSources {
            style_sheets: vec!["first.css".into(), "second.css".into()],
            ..ViewSources::new("main.js")
        },
    )
    .await;
    boot(&mut view, &receiver).unwrap();
    let screenshot = view.capture().unwrap();
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
        let mut view = view(&group, &resources, sources).await;
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
    let mut failed = view(&group, &resources, ViewSources::new("missing.js")).await;
    assert!(matches!(
        boot(&mut failed, &receiver),
        Err(LynxViewError::Resource(_))
    ));
    resources
        .register("app:///main.js", "", Some("text/javascript"))
        .unwrap();
    let mut sibling = view(&group, &resources, ViewSources::new("main.js")).await;
    boot(&mut sibling, &receiver).unwrap();
    sibling.tick(true).unwrap();
    assert!(failed.pump().is_empty());
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
    let mut first = view(
        &group,
        &first_resources,
        ViewSources::new("nested/first.js"),
    )
    .await;
    let mut second = view(
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
        for (view, color) in [
            (&mut first, [0, 0, 255, 255]),
            (&mut second, [255, 0, 0, 255]),
        ] {
            for event in view.pump() {
                match event {
                    EngineEvent::StartupFailed(error) => panic!("startup: {error}"),
                    EngineEvent::WorkerFailed(error)
                    | EngineEvent::ListenerFailed(error)
                    | EngineEvent::ScriptRunError(error) => panic!("script: {error}"),
                    EngineEvent::RenderFailed(error) => panic!("render: {error}"),
                    _ => {}
                }
            }
            let screenshot = view.capture().unwrap();
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
async fn xml_background_uses_bts_bootstrap_and_defers_application_module_loading() {
    let (group, resources, receiver) = setup().await;
    let page = PageSource::from_bytes(
        &"app:///card.lynx.xml".parse().unwrap(),
        br#"
        <lynx engine-version="4.2">
          <script thread="main"><![CDATA[
            __CreatePage();
            lynx.getJSContext().dispatchEvent({type: 'initialize'});
          ]]></script>
          <script thread="background"><![CDATA[
            throw Error('XML body was executed without being imported');
          ]]></script>
        </lynx>
        "#,
    )
    .unwrap();
    page.register_with(&resources);
    let sources = page.view_sources();
    let background_url = sources.background_entry.clone().unwrap();
    let mut view = view(&group, &resources, sources).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut script_finished = false;
    let mut import_failed = false;
    while !script_finished || !import_failed {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => script_finished = true,
                EngineEvent::WorkerFailed(error) => {
                    // The bootstrap imports the XML entry. Loading that module
                    // from ResourceFetcher is explicitly deferred in this MVP.
                    assert!(!import_failed, "duplicate import failure");
                    assert!(error.message.contains(&background_url), "{error}");
                    assert!(error.message.contains("not preloaded"), "{error}");
                    import_failed = true;
                }
                EngineEvent::StartupFailed(error) => panic!("startup: {error}"),
                EngineEvent::ListenerFailed(error) | EngineEvent::ScriptRunError(error) => {
                    panic!("script: {error}")
                }
                EngineEvent::RenderFailed(error) => panic!("render: {error}"),
                _ => {}
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "BTS import did not report its outcome"
        );
        let _ = receiver.recv_timeout(Duration::from_millis(5));
    }
}
