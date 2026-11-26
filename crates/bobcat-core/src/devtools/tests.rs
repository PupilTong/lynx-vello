//! The devtools attachment over a real view: refusal, one frontend at a
//! time, the envelope's errors, worker sessions and the end of a view.

use std::sync::Arc;

use serde_json::json;

use super::test_support::{Client, Recorder, attach, booted};
use crate::test_support::{TestViewSpec, wait_for_boot};
use crate::view::{EngineError, NoWakeup};

const PAGE: &str = r"
globalThis.renderPage = function () {
  __CreatePage('card', 0);
};
";

const WORKERS_PAGE: &str = r"
import { Worker } from 'bobcat-internal';
globalThis.renderPage = function () {
  __CreatePage('card', 0);
};
globalThis.updatePage = function (data) {
  if (data.start) globalThis.worker = new Worker('./w.js');
  else globalThis.worker.terminate();
};
";

#[test]
fn a_view_created_without_the_switch_refuses() {
    let engine = TestViewSpec::new(PAGE).boot();
    let refused = engine.view.devtools_attach(Arc::new(Recorder::default()));
    assert!(matches!(refused, Err(EngineError::DevtoolsDisabled)));
}

#[test]
fn one_frontend_at_a_time_and_a_dropped_one_hears_nothing_more() {
    let engine = booted(TestViewSpec::new(PAGE));
    let (first, session) = attach(&engine);
    assert!(matches!(
        engine.view.devtools_attach(Arc::new(Recorder::default())),
        Err(EngineError::DevtoolsAttached)
    ));
    session.send(r#"{"id":1,"method":"Page.enable"}"#.to_owned());
    first.answer(1);
    drop(session);

    let (second, session) = attach(&engine);
    session.send(r#"{"id":2,"method":"Page.enable"}"#.to_owned());
    second.answer(2);
    assert_eq!(first.count(), 1);
    assert_eq!(first.closed(), 0);
    drop(session);
}

#[test]
fn the_envelope_errors_carry_cdp_codes() {
    let engine = booted(TestViewSpec::new(PAGE));
    let (recorder, session) = attach(&engine);

    session.send(r#"{"id":7,"method":"Page.enable"}"#.to_owned());
    let answer = recorder.answer(7);
    assert_eq!(answer["error"]["code"], -32601);
    assert_eq!(answer["error"]["message"], "'Page.enable' wasn't found");

    session.send("not json".to_owned());
    let (answer, _) = recorder.wait_for("parse error", |value| value["error"]["code"] == -32700);
    assert!(answer.get("id").is_none(), "{answer}");

    session.send(r#"{"id":8,"method":"Runtime.enable","sessionId":"nobody"}"#.to_owned());
    let answer = recorder.answer(8);
    assert_eq!(answer["error"]["code"], -32001);
    assert_eq!(
        answer["error"]["message"],
        "Session with given id not found."
    );
}

#[test]
fn messages_sent_before_the_view_is_ready_are_answered_after_boot() {
    let mut engine = TestViewSpec::new(PAGE)
        .with_devtools()
        .create(Arc::new(NoWakeup));
    let (recorder, session) = attach(&engine);
    session.send(r#"{"id":3,"method":"Page.enable"}"#.to_owned());
    wait_for_boot(&mut engine);
    let (answer, thread) = recorder.wait_for("answer", |value| value["id"] == 3);
    assert_eq!(answer["error"]["code"], -32601);
    assert_eq!(thread.as_deref(), Some("bobcat-main"));
}

#[test]
fn auto_attach_announces_workers_and_routes_their_sessions() {
    let mut engine = booted(
        TestViewSpec::new(WORKERS_PAGE)
            .with_background_entry("globalThis.started = true;")
            .with_module("app:///w.js", "import 'bobcat:worker';"),
    );
    let client = Client::attach(&engine);
    let auto_attach = json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true});
    assert_eq!(
        client.result(&mut engine, None, "Target.setAutoAttach", auto_attach),
        json!({})
    );
    let bts = client.event(&mut engine, None, "Target.attachedToTarget");
    let info = &bts["targetInfo"];
    assert_eq!(info["url"], "bobcat:bts");
    assert_eq!(info["type"], "worker");
    assert_eq!(info["title"], "bobcat:bts");
    assert_eq!(info["targetId"], bts["sessionId"]);
    assert_eq!(bts["waitingForDebugger"], false);

    engine
        .view
        .update_data(r#"{"start":true}"#.to_owned(), String::new())
        .unwrap();
    let worker = client.event(&mut engine, None, "Target.attachedToTarget");
    assert_eq!(worker["targetInfo"]["url"], "app:///w.js");
    let session_id = worker["sessionId"].as_str().unwrap().to_owned();

    let error = client.error(&mut engine, Some(&session_id), "DOM.getDocument", json!({}));
    assert_eq!(error["code"], -32601);
    let (answer, thread) = client
        .recorder
        .wait_for("the worker's answer", |value| value["error"] == error);
    assert_eq!(answer["sessionId"], session_id.as_str());
    assert_eq!(thread.as_deref(), Some("bobcat-workers"));

    engine
        .view
        .update_data(r#"{"start":false}"#.to_owned(), String::new())
        .unwrap();
    let detached = client.event(&mut engine, None, "Target.detachedFromTarget");
    assert_eq!(detached["sessionId"], session_id.as_str());
    assert_eq!(detached["targetId"], session_id.as_str());
}

#[test]
fn set_auto_attach_without_flatten_is_invalid_params() {
    let mut engine = booted(TestViewSpec::new(PAGE));
    let client = Client::attach(&engine);
    let error = client.error(
        &mut engine,
        None,
        "Target.setAutoAttach",
        json!({"autoAttach": true, "waitForDebuggerOnStart": false}),
    );
    assert_eq!(error["code"], -32602);
}

#[test]
fn dropping_the_view_while_attached_closes_the_sink_once() {
    let engine = booted(TestViewSpec::new(PAGE));
    let (recorder, session) = attach(&engine);
    drop(engine);
    assert_eq!(recorder.closed(), 1);
    drop(session);
    assert_eq!(recorder.closed(), 1);
}

#[test]
fn dropping_the_session_does_not_close_the_sink() {
    let engine = booted(TestViewSpec::new(PAGE));
    let (recorder, session) = attach(&engine);
    drop(session);
    drop(engine);
    assert_eq!(recorder.closed(), 0);
}

#[test]
fn dropping_a_view_right_after_attaching_closes_the_sink_once() {
    let engine = TestViewSpec::new(PAGE)
        .with_devtools()
        .create(Arc::new(NoWakeup));
    let (recorder, session) = attach(&engine);
    drop(engine);
    assert_eq!(recorder.closed(), 1);
    drop(session);
}
