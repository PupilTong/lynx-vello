//! Public host updates require MTS boot and reach a loading BTS in order.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{
    DrawTarget, EngineError, EngineEvent, LynxGroup, LynxView, NoWakeup, Painter, StyleThreads,
};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use serde_json::json;
use url::Url;

mod support;
use support::DelayedBackground;

#[derive(Default)]
struct Observed {
    messages: Vec<String>,
    booted: bool,
}

async fn until(view: &mut LynxView<DelayedBackground>, seen: &mut Observed, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !seen.messages.iter().any(|message| message == expected) || !view.is_ready() {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => seen.booted = true,
                EngineEvent::ConsoleMessage { message, .. } => seen.messages.push(message),
                other => panic!("unexpected lifecycle event: {other:?}"),
            }
        }
        assert!(
            Instant::now() < deadline,
            "waiting for {expected}: {:?}",
            seen.messages
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn reject_updates(view: &LynxView<DelayedBackground>) {
    assert!(matches!(
        view.update_data(json!({"raw":999}).to_string(), String::new()),
        Err(EngineError::NotReady)
    ));
    assert!(matches!(
        view.reset_data(json!({"raw":999}).to_string(), String::new()),
        Err(EngineError::NotReady)
    ));
    assert!(matches!(
        view.update_global_props(json!({"theme":"early"}).to_string()),
        Err(EngineError::NotReady)
    ));
    assert!(matches!(
        view.reload(json!({"raw":999}).to_string(), String::new()),
        Err(EngineError::NotReady)
    ));
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one public-API sequence pins ordering across the delayed BTS boot and both reload origins"
)]
async fn public_updates_require_mts_boot_then_preserve_order() {
    let page = PageSource::from_bytes(&Url::parse("app:///lifecycle.xml").unwrap(), br#"
      <lynx engine-version="4.1"><script thread="main">
        if (SystemInfo.pixelRatio !== 1 || SystemInfo.pixelWidth !== 100 || SystemInfo.pixelHeight !== 100)
          throw Error('MTS system info');
        globalThis.executions = (globalThis.executions || 0) + 1;
        globalThis.processData = (data, name) => {
          if (name !== '') throw Error('default processor name');
          const result = {count:data.raw + 1};
          Promise.resolve().then(() => { result.finished = true; });
          return result;
        };
        globalThis.renderPage = data => {
          if (data.finished) throw Error('processor jobs ran before render');
          console.log('mts render ' + data.count);
          __OnLifecycleEvent(['first-screen', data.count]);
        };
        globalThis.removeComponents = () => console.log('mts remove');
        globalThis.updatePage = (data, options) => {
          if (executions !== 1 || options.nativeUpdateDataOrder !== 0) throw Error('entry or options');
          console.log('mts update ' + data.count + ' ' + options.resetPageData + ' ' + options.reloadFromJS);
          if (options.reloadTemplate) __OnLifecycleEvent(['first-screen', data.count]);
        };
        globalThis.updateGlobalProps = props => {
          if (props !== lynx.__globalProps || props !== __globalProps || props.keep !== 1) throw Error('MTS global props');
          console.log('mts props ' + props.theme);
        };
      </script><script thread="background">
        import {lynx as backgroundLynx, console as backgroundConsole} from 'bobcat:bts-runtime';
        if (backgroundLynx.SystemInfo.pixelRatio !== 1 || backgroundLynx.SystemInfo.pixelWidth !== 100 || backgroundLynx.SystemInfo.pixelHeight !== 100)
          throw Error('BTS system info');
        const app = backgroundLynx.getApp();
        if (app._params.initData !== null || app._params.processorName !== '' || app._params.cacheData.length)
          throw Error('initial data slots');
        if (app._params.updateData !== backgroundLynx.__initData || backgroundLynx.__initData.finished || backgroundLynx.__globalProps.theme !== 'light')
          throw Error('initial snapshot');
        backgroundConsole.log('bts initial ' + backgroundLynx.__initData.count);
        app.OnLifecycleEvent = ([name, count]) => backgroundConsole.log('bts ' + name + ' ' + count);
        app.updateCardData = (data, options) => backgroundConsole.log('bts update ' + data.count + ' ' + options.type);
        app.updateGlobalProps = props => backgroundConsole.log('bts props ' + props.theme + ' ' + props.keep);
        app.onAppReload = data => backgroundConsole.log('bts reload ' + data.count);
        backgroundLynx.getJSModule('GlobalEventEmitter').addListener('reload-from-bts', () => {
          backgroundLynx.reload({count:9}, () => backgroundConsole.log('bts callback'));
        });
      </script></lynx>
    "#).unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let released = Rc::new(Cell::new(false));
    let pending = Rc::default();
    let mut sources = page.view_sources();
    sources.init_data = Some(json!({"raw":1}).to_string());
    sources.global_props = Some(json!({"theme":"light","keep":1}).to_string());
    let background = sources.background_entry.clone().unwrap();
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(
            100.0,
            100.0,
            1.0,
            |reports| DelayedBackground {
                resources: resources.builder()(reports),
                background,
                pending,
                released: Rc::clone(&released),
            },
            Vec::new(),
            sources,
        )
        .unwrap();
    // Boot's first flush waits for a painter to bind the view, and everything
    // below is past it.
    let mut painter = Painter::new(DrawTarget::Offscreen, 100.0, 100.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();
    reject_updates(&view);
    let mut seen = Observed::default();
    until(&mut view, &mut seen, "mts render 2").await;
    assert!(seen.booted);
    assert!(view.is_ready());
    assert!(!released.get());
    view.update_data(json!({"raw":2}).to_string(), String::new())
        .unwrap();
    until(&mut view, &mut seen, "mts update 3 false false").await;
    assert!(
        !seen
            .messages
            .iter()
            .any(|message| message.starts_with("bts ")),
        "the held BTS entry must not have run yet: {:?}",
        seen.messages
    );
    released.set(true);
    until(&mut view, &mut seen, "bts update 3 0").await;
    view.reset_data(json!({"raw":3}).to_string(), String::new())
        .unwrap();
    view.update_global_props(json!({"theme":"dark"}).to_string())
        .unwrap();
    view.reload(json!({"raw":4}).to_string(), String::new())
        .unwrap();
    until(&mut view, &mut seen, "bts first-screen 5").await;
    assert_eq!(
        seen.messages
            .iter()
            .filter(|message| message.starts_with("bts "))
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "bts initial 2",
            "bts first-screen 2",
            "bts update 3 0",
            "bts update 4 1",
            "bts props dark 1",
            "bts reload 5",
            "bts first-screen 5",
        ]
    );
    view.send_global_event("reload-from-bts", "[]".into())
        .unwrap();
    until(&mut view, &mut seen, "bts callback").await;
    assert!(
        seen.messages
            .iter()
            .any(|message| message == "mts update 9 false true")
    );
    assert!(seen.messages.ends_with(&[
        "bts reload 9".to_owned(),
        "bts first-screen 9".to_owned(),
        "bts callback".to_owned(),
    ]));
}
