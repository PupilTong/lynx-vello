//! Public host updates retain their order while the BTS entry is loading.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{EngineEvent, LynxGroup, LynxView, NoWakeup, StyleThreads};
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

async fn until(
    view: &mut LynxView<DelayedBackground>,
    seen: &mut Observed,
    expected: &str,
    require_ready: bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !seen.messages.iter().any(|message| message == expected)
        || (require_ready && !view.is_ready())
    {
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

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one public-API sequence pins ordering across the delayed BTS boot and both reload origins"
)]
async fn public_updates_and_reload_preserve_order_through_a_delayed_background_entry() {
    let page = PageSource::from_bytes(&Url::parse("app:///lifecycle.xml").unwrap(), br#"
      <lynx engine-version="4.1"><script thread="main">
        globalThis.executions = (globalThis.executions || 0) + 1;
        globalThis.processData = (data, name) => {
          if (name !== '') throw Error('default processor name');
          const result = {count:data.raw + 1};
          Promise.resolve().then(() => { result.finished = true; });
          return result;
        };
        globalThis.renderPage = data => {
          if (!data.finished) throw Error('unfinished initial processor');
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
        const app = backgroundLynx.getApp();
        if (app._params.initData !== null || app._params.processorName !== '' || app._params.cacheData.length)
          throw Error('initial data slots');
        if (app._params.updateData !== backgroundLynx.__initData || !backgroundLynx.__initData.finished || backgroundLynx.__globalProps.theme !== 'light')
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
            sources,
        )
        .unwrap();
    let mut seen = Observed::default();
    until(&mut view, &mut seen, "mts render 2", false).await;
    assert!(!view.is_ready());
    view.update_data(json!({"raw":2}).as_object().unwrap().clone());
    view.reset_data(json!({"raw":3}).as_object().unwrap().clone());
    view.update_global_props(json!({"theme":"dark"}).as_object().unwrap().clone());
    view.reload(json!({"raw":4}).as_object().unwrap().clone());
    until(&mut view, &mut seen, "mts update 5 false false", false).await;
    assert!(!seen.booted);
    released.set(true);
    until(&mut view, &mut seen, "bts first-screen 5", true).await;
    assert!(seen.booted);
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
    view.send_global_event("reload-from-bts", Vec::new())
        .unwrap();
    until(&mut view, &mut seen, "bts callback", true).await;
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
