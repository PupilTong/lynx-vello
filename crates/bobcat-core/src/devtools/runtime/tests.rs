//! The `Runtime` domain over a real view: remote objects, properties,
//! evaluation, console capture, the worker session and the release of a
//! realm with held objects.

use std::cell::Cell;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::devtools::test_support::{Client, booted};
use crate::test_support::{TestEngine, TestViewSpec, wait_for_boot};
use crate::view::{EngineEvent, NoWakeup};

static TAPS: Mutex<Vec<(String, Weak<()>)>> = Mutex::new(Vec::new());

pub(super) fn register_tap(unique_id: &str, live: &Arc<()>) {
    TAPS.lock()
        .unwrap()
        .push((unique_id.to_owned(), Arc::downgrade(live)));
}

fn tap_is_live(unique_id: &str) -> bool {
    TAPS.lock()
        .unwrap()
        .iter()
        .any(|(id, live)| id == unique_id && live.strong_count() > 0)
}

const PAGE: &str = r"
globalThis.renderPage = function () {
  __CreatePage('card', 0);
};
globalThis.con = console;
globalThis.saved = console.log;
globalThis.report = _ReportError;
globalThis.updatePage = function () {
  con.log('updated');
};
";

const PATIENCE: Duration = Duration::from_secs(30);

struct Page {
    engine: TestEngine,
    client: Client,
    next: Cell<i64>,
}

impl Page {
    fn new() -> Self {
        Self::with(TestViewSpec::new(PAGE))
    }

    fn with(spec: TestViewSpec) -> Self {
        let engine = booted(spec);
        let client = Client::attach(&engine);
        Self {
            engine,
            client,
            next: Cell::new(1),
        }
    }

    fn result(&mut self, method: &str, params: Value) -> Value {
        self.client.result(&mut self.engine, None, method, params)
    }

    fn error(&mut self, method: &str, params: Value) -> Value {
        self.client.error(&mut self.engine, None, method, params)
    }

    fn evaluate(&mut self, expression: &str) -> Value {
        self.result(
            "Runtime.evaluate",
            json!({"expression": expression, "generatePreview": true}),
        )["result"]
            .clone()
    }

    fn properties(&mut self, object_id: &Value, params: Value) -> Value {
        let mut params = params;
        params["objectId"] = object_id.clone();
        self.result("Runtime.getProperties", params)
    }

    fn enable(&mut self, session_id: Option<&str>) -> Value {
        self.client
            .result(&mut self.engine, session_id, "Runtime.enable", json!({}));
        self.client.event(
            &mut self.engine,
            session_id,
            "Runtime.executionContextCreated",
        )
    }

    fn evaluate_unpumped(&self, session_id: Option<&str>, expression: &str) -> Value {
        let id = self.next.get();
        self.next.set(id + 1);
        let mut message = json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": {"expression": expression},
        });
        if let Some(session_id) = session_id {
            message["sessionId"] = json!(session_id);
        }
        self.client.session.send(message.to_string());
        self.client.recorder.answer(id)
    }

    fn console(&mut self, count: usize) -> Vec<(String, String)> {
        let deadline = Instant::now() + PATIENCE;
        let mut messages = Vec::new();
        while messages.len() < count {
            assert!(Instant::now() < deadline, "only {messages:?} arrived");
            for event in self.engine.pump() {
                if let EngineEvent::ConsoleMessage { level, message, .. }
                | EngineEvent::ScriptReported { level, message, .. } = event
                {
                    messages.push((level, message));
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        messages
    }

    fn engine_event(&mut self, what: &str, matches: impl Fn(&EngineEvent) -> bool) -> EngineEvent {
        let deadline = Instant::now() + PATIENCE;
        loop {
            assert!(Instant::now() < deadline, "no {what} arrived");
            if let Some(event) = self.engine.pump().into_iter().find(&matches) {
                return event;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn bts_session(&mut self) -> String {
        self.result(
            "Target.setAutoAttach",
            json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true}),
        );
        let attached = self
            .client
            .event(&mut self.engine, None, "Target.attachedToTarget");
        assert_eq!(attached["targetInfo"]["url"], "bobcat:bts");
        attached["sessionId"].as_str().unwrap().to_owned()
    }
}

#[test]
fn primitives_and_special_numbers() {
    let mut page = Page::new();
    assert_eq!(page.evaluate("undefined"), json!({"type": "undefined"}));
    assert_eq!(
        page.evaluate("null"),
        json!({"type": "object", "subtype": "null", "value": null})
    );
    assert_eq!(
        page.evaluate("true"),
        json!({"type": "boolean", "value": true})
    );
    assert_eq!(
        page.evaluate("'text'"),
        json!({"type": "string", "value": "text"})
    );
    assert_eq!(
        page.evaluate("1.5"),
        json!({"type": "number", "value": 1.5, "description": "1.5"})
    );
    assert_eq!(
        page.evaluate("42"),
        json!({"type": "number", "value": 42, "description": "42"})
    );
    for special in ["NaN", "Infinity", "-Infinity", "-0"] {
        assert_eq!(
            page.evaluate(special),
            json!({"type": "number", "unserializableValue": special, "description": special})
        );
    }
    assert_eq!(
        page.evaluate("12345678901234567890n"),
        json!({
            "type": "bigint",
            "unserializableValue": "12345678901234567890n",
            "description": "12345678901234567890n",
        })
    );
    let symbol = page.evaluate("Symbol('tag')");
    assert_eq!(symbol["type"], "symbol");
    assert_eq!(symbol["description"], "Symbol(tag)");
    assert!(symbol["objectId"].is_string());
}

#[test]
fn a_plain_object_is_previewed() {
    let mut page = Page::new();
    let plain = page.evaluate("({ a: 1, b: 'two', c: { d: 1 }, get e() { return 1; } })");
    assert_eq!(plain["type"], "object");
    assert!(plain.get("subtype").is_none());
    assert_eq!(plain["className"], "Object");
    assert_eq!(plain["description"], "Object");
    assert!(plain["objectId"].is_string());
    assert_eq!(
        plain["preview"],
        json!({
            "type": "object",
            "description": "Object",
            "overflow": false,
            "properties": [
                {"name": "a", "type": "number", "value": "1"},
                {"name": "b", "type": "string", "value": "two"},
                {"name": "c", "type": "object", "value": "Object"},
                {"name": "e", "type": "accessor"},
            ],
        })
    );
    let many = page.evaluate("({ a: 1, b: 2, c: 3, d: 4, e: 5, f: 6 })");
    assert_eq!(many["preview"]["overflow"], true);
    assert_eq!(many["preview"]["properties"].as_array().unwrap().len(), 5);
}

#[test]
fn built_in_objects_are_described_by_class() {
    let mut page = Page::new();
    let array = page.evaluate("[1, 'x', null]");
    assert_eq!(
        (
            &array["subtype"],
            &array["className"],
            &array["description"]
        ),
        (&json!("array"), &json!("Array"), &json!("Array(3)"))
    );
    assert_eq!(
        array["preview"]["properties"][2],
        json!({"name": "2", "type": "object", "subtype": "null", "value": "null"})
    );
    let typed = page.evaluate("new Uint8Array(4)");
    assert_eq!(typed["subtype"], "typedarray");
    assert_eq!(typed["className"], "Uint8Array");
    assert_eq!(typed["description"], "Uint8Array(4)");

    let error = page.evaluate("new RangeError('bad')");
    assert_eq!(error["subtype"], "error");
    assert_eq!(error["className"], "RangeError");
    assert!(
        error["description"]
            .as_str()
            .unwrap()
            .starts_with("RangeError: bad")
    );
    let date = page.evaluate("new Date(0)");
    assert_eq!(
        (&date["subtype"], &date["className"], &date["description"]),
        (
            &json!("date"),
            &json!("Date"),
            &json!("1970-01-01T00:00:00.000Z")
        )
    );
    let regexp = page.evaluate("/a+/g");
    assert_eq!(
        (&regexp["subtype"], &regexp["description"]),
        (&json!("regexp"), &json!("/a+/g"))
    );
    let map = page.evaluate("new Map([['k', 1]])");
    assert_eq!(
        (&map["subtype"], &map["className"]),
        (&json!("map"), &json!("Map"))
    );
    assert!(map["description"].as_str().unwrap().starts_with("Map(1)"));
    let set = page.evaluate("new Set([1, 2])");
    assert_eq!(set["subtype"], "set");
    assert!(set["description"].as_str().unwrap().starts_with("Set(2)"));
    for promise in [
        "Promise.resolve(1)",
        "new Promise(() => {})",
        "Promise.reject(1)",
    ] {
        let promise = page.evaluate(promise);
        assert_eq!(
            (
                &promise["subtype"],
                &promise["className"],
                &promise["description"]
            ),
            (&json!("promise"), &json!("Promise"), &json!("Promise"))
        );
    }
    let function = page.evaluate("(function named(a) {})");
    assert_eq!(
        (
            &function["type"],
            &function["className"],
            &function["description"]
        ),
        (
            &json!("function"),
            &json!("Function"),
            &json!("function named()")
        )
    );
    assert!(function["objectId"].is_string());
    let instance = page.evaluate("class Point {}; new Point()");
    assert_eq!(instance["className"], "Point");
    assert_eq!(instance["description"], "Point");
    let bare = page.evaluate("Object.create(null)");
    assert_eq!(bare["className"], "Object");
}

#[test]
fn promises_expose_their_state_as_internal_properties() {
    let mut page = Page::new();
    for (source, state, result) in [
        (
            "Promise.resolve(7)",
            "fulfilled",
            json!({"type": "number", "value": 7, "description": "7"}),
        ),
        (
            "new Promise(() => {})",
            "pending",
            json!({"type": "undefined"}),
        ),
        (
            "Promise.reject('no')",
            "rejected",
            json!({"type": "string", "value": "no"}),
        ),
    ] {
        let promise = page.evaluate(source);
        let properties = page.properties(&promise["objectId"], json!({"ownProperties": true}));
        let internal = properties["internalProperties"].as_array().unwrap();
        let names: Vec<_> = internal.iter().map(|property| &property["name"]).collect();
        assert_eq!(
            names,
            [
                &json!("[[Prototype]]"),
                &json!("[[PromiseState]]"),
                &json!("[[PromiseResult]]")
            ]
        );
        assert_eq!(
            internal[1]["value"],
            json!({"type": "string", "value": state})
        );
        assert_eq!(internal[2]["value"], result);
    }
}

#[test]
fn properties_read_own_inherited_accessors_and_symbols_without_running_getters() {
    let mut page = Page::new();
    page.evaluate(
        "globalThis.getterCalls = 0;
         globalThis.sample = Object.create({ inherited: 1, shadowed: 0 }, {
           own: { value: 2, writable: true, enumerable: true, configurable: true },
           shadowed: { value: 3, enumerable: true },
           computed: { get() { getterCalls++; return 4; }, enumerable: true },
           [Symbol('mark')]: { value: 5, enumerable: true },
         });",
    );
    let sample = page.evaluate("sample");
    let own = page.properties(&sample["objectId"], json!({"ownProperties": true}));
    let result = own["result"].as_array().unwrap();
    let named = |name: &str| {
        result
            .iter()
            .find(|property| property["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {result:?}"))
            .clone()
    };
    let own_value = named("own");
    assert_eq!(own_value["value"]["value"], 2);
    assert_eq!(
        (
            &own_value["writable"],
            &own_value["enumerable"],
            &own_value["configurable"],
            &own_value["isOwn"]
        ),
        (&json!(true), &json!(true), &json!(true), &json!(true))
    );
    assert_eq!(named("shadowed")["writable"], false);
    let computed = named("computed");
    assert_eq!(computed["get"]["type"], "function");
    assert_eq!(computed["set"], json!({"type": "undefined"}));
    assert!(computed.get("value").is_none());
    let symbol = named("Symbol(mark)");
    assert_eq!(symbol["symbol"]["type"], "symbol");
    assert_eq!(symbol["value"]["value"], 5);
    assert!(
        result
            .iter()
            .all(|property| property["name"] != "inherited")
    );
    assert_eq!(
        own["internalProperties"][0]["name"], "[[Prototype]]",
        "{own}"
    );

    let all = page.properties(&sample["objectId"], json!({}));
    let all = all["result"].as_array().unwrap();
    let inherited = all
        .iter()
        .find(|property| property["name"] == "inherited")
        .unwrap();
    assert_eq!(inherited["isOwn"], false);
    assert_eq!(
        all.iter()
            .filter(|property| property["name"] == "shadowed")
            .count(),
        1
    );
    assert!(
        all.iter()
            .any(|property| property["name"] == "hasOwnProperty")
    );

    let accessors = page.properties(
        &sample["objectId"],
        json!({"ownProperties": true, "accessorPropertiesOnly": true}),
    );
    let accessors = accessors["result"].as_array().unwrap();
    assert_eq!(accessors.len(), 1);
    assert_eq!(accessors[0]["name"], "computed");

    assert_eq!(page.evaluate("getterCalls")["value"], 0);
}

#[test]
fn non_indexed_properties_of_a_large_array() {
    let mut page = Page::new();
    let array = page.evaluate("const big = new Array(100000).fill(1); big.extra = 'x'; big");
    assert_eq!(array["description"], "Array(100000)");
    assert_eq!(
        array["preview"]["properties"].as_array().unwrap().len(),
        100
    );
    assert_eq!(array["preview"]["overflow"], true);
    let properties = page.properties(
        &array["objectId"],
        json!({"ownProperties": true, "nonIndexedPropertiesOnly": true}),
    );
    let mut names: Vec<_> = properties["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|property| property["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["extra", "length"]);
}

const TRAPPED: &str = "
globalThis.traps = 0;
globalThis.proxy = new Proxy({ a: 1 }, {
  get(target, key) { traps++; return target[key]; },
  has(target, key) { traps++; return key in target; },
  ownKeys(target) { traps++; return Reflect.ownKeys(target); },
  getOwnPropertyDescriptor(target, key) { traps++; return Reflect.getOwnPropertyDescriptor(target, key); },
  getPrototypeOf(target) { traps++; return Object.getPrototypeOf(target); },
});
";

#[test]
fn a_proxy_is_reported_and_none_of_its_traps_run() {
    let mut page = Page::new();
    page.evaluate(TRAPPED);
    let unwatched = page.evaluate("traps = 0; con.log(proxy); traps")["value"].clone();
    assert_ne!(unwatched, json!(0));
    page.enable(None);
    let proxy = page.evaluate("traps = 0; proxy");
    assert_eq!(
        (
            &proxy["type"],
            &proxy["subtype"],
            &proxy["className"],
            &proxy["description"]
        ),
        (
            &json!("object"),
            &json!("proxy"),
            &json!("Object"),
            &json!("Proxy")
        )
    );
    assert_eq!(proxy["preview"]["properties"], json!([]));
    let properties = page.properties(&proxy["objectId"], json!({"generatePreview": true}));
    assert_eq!(properties, json!({"result": []}));
    let class = page.evaluate("class Wrapped {}; Object.setPrototypeOf({}, proxy)");
    assert_eq!(class["className"], "Object");
    assert_eq!(page.evaluate("traps")["value"], 0);

    let watched = page.evaluate("traps = 0; con.log(proxy); traps")["value"].clone();
    assert_eq!(watched, unwatched);
    let logged = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(logged["args"][0]["subtype"], "proxy");
}

#[test]
fn call_function_on_takes_this_and_every_argument_form() {
    let mut page = Page::new();
    let target = page.evaluate("({ x: 2 })");
    let other = page.evaluate("({ y: 7 })");
    let called = page.result(
        "Runtime.callFunctionOn",
        json!({
            "functionDeclaration": "function (n, o, nan, big, nul, negz, none) {
                return { x: this.x, n, o: o.y, nan: Number.isNaN(nan), big: big === 5n,
                         nul: nul === null, negz: Object.is(negz, -0), none: none === undefined };
            }",
            "objectId": target["objectId"],
            "arguments": [
                {"value": 3},
                {"objectId": other["objectId"]},
                {"unserializableValue": "NaN"},
                {"unserializableValue": "5n"},
                {"value": null},
                {"unserializableValue": "-0"},
                {},
            ],
            "returnByValue": true,
        }),
    );
    assert_eq!(
        called,
        json!({"result": {"type": "object", "value": {
            "x": 2, "n": 3, "o": 7, "nan": true, "big": true, "nul": true, "negz": true, "none": true,
        }}})
    );

    let held = page.result(
        "Runtime.callFunctionOn",
        json!({
            "functionDeclaration": "function () { return [this.x]; }",
            "objectId": target["objectId"],
            "generatePreview": true,
        }),
    );
    assert_eq!(held["result"]["description"], "Array(1)");
    assert!(held["result"]["objectId"].is_string());
    let unbound = page.result(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration": "function () { 'use strict'; return this === undefined; }"}),
    );
    assert_eq!(unbound["result"]["value"], json!(true));
    let by_value = page.result(
        "Runtime.evaluate",
        json!({"expression": "({ toJSON() { return [1]; } })", "returnByValue": true}),
    );
    assert_eq!(by_value["result"], json!({"type": "object", "value": [1]}));
    let nothing = page.result(
        "Runtime.evaluate",
        json!({"expression": "(function () {})", "returnByValue": true}),
    );
    assert_eq!(nothing["result"], json!({"type": "undefined"}));
}

#[test]
fn a_throw_answers_exception_details() {
    let mut page = Page::new();
    let thrown = page.result(
        "Runtime.evaluate",
        json!({"expression": "\n  throw new TypeError('boom');"}),
    );
    let result = &thrown["result"];
    assert_eq!(
        (&result["subtype"], &result["className"]),
        (&json!("error"), &json!("TypeError"))
    );
    assert!(
        result["description"]
            .as_str()
            .unwrap()
            .starts_with("TypeError: boom")
    );
    assert!(result.get("objectId").is_none());
    let details = &thrown["exceptionDetails"];
    assert_eq!(details["text"], "Uncaught");
    assert_eq!(details["exception"], *result);
    assert_eq!(details["lineNumber"], 1);
    assert_eq!(details["executionContextId"], 1);
    let id = details["exceptionId"].as_i64().unwrap();

    let syntax = page.result("Runtime.evaluate", json!({"expression": "("}));
    assert_eq!(
        syntax["exceptionDetails"]["exception"]["className"],
        "SyntaxError"
    );
    assert_eq!(syntax["exceptionDetails"]["exceptionId"], id + 1);
    let thrown_call = page.result(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration": "function () { throw 'plain'; }"}),
    );
    assert_eq!(
        thrown_call["exceptionDetails"]["exception"],
        json!({"type": "string", "value": "plain"})
    );
}

#[test]
fn a_side_effect_check_and_await_promise_are_refused() {
    let mut page = Page::new();
    let refused = page.result(
        "Runtime.evaluate",
        json!({"expression": "globalThis.sideEffect = 1", "throwOnSideEffect": true}),
    );
    let error = json!({
        "type": "object",
        "subtype": "error",
        "className": "EvalError",
        "description": "EvalError: Possible side-effect in debug-evaluate",
    });
    assert_eq!(refused["result"], error);
    assert_eq!(refused["exceptionDetails"]["exception"], error);
    assert_eq!(page.evaluate("typeof sideEffect")["value"], "undefined");
    let call = page.result(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration": "function () { globalThis.sideEffect = 1; }", "throwOnSideEffect": true}),
    );
    assert_eq!(call["result"], error);
    assert_eq!(page.evaluate("typeof sideEffect")["value"], "undefined");

    let awaiting = page.error(
        "Runtime.evaluate",
        json!({"expression": "1", "awaitPromise": true}),
    );
    assert_eq!(awaiting["code"], -32000);
    let wrong_context = page.error(
        "Runtime.evaluate",
        json!({"expression": "1", "contextId": 2}),
    );
    assert_eq!(wrong_context["code"], -32000);
}

#[test]
fn objects_are_released_by_id_and_by_group() {
    let mut page = Page::new();
    let grouped = |page: &mut Page| {
        page.result(
            "Runtime.evaluate",
            json!({"expression": "({})", "objectGroup": "g"}),
        )["result"]["objectId"]
            .clone()
    };
    let first = grouped(&mut page);
    let second = grouped(&mut page);
    let kept = page.evaluate("({})")["objectId"].clone();
    page.result("Runtime.releaseObject", json!({"objectId": first}));
    let missing = page.error("Runtime.getProperties", json!({"objectId": first}));
    assert_eq!(
        missing,
        json!({"code": -32000, "message": "Could not find object with given id"})
    );
    page.result("Runtime.releaseObjectGroup", json!({"objectGroup": "g"}));
    assert_eq!(
        page.error("Runtime.getProperties", json!({"objectId": second}))["code"],
        -32000
    );
    page.properties(&kept, json!({}));
    assert_eq!(
        page.error("Runtime.releaseObject", json!({"objectId": "nonsense"}))["code"],
        -32000
    );
    page.result("Runtime.disable", json!({}));
    assert_eq!(
        page.error("Runtime.getProperties", json!({"objectId": kept}))["code"],
        -32000
    );
}

#[test]
fn a_second_enable_announces_nothing() {
    let mut page = Page::new();
    let created = page.enable(None);
    assert_eq!(created["context"]["id"], 1);
    assert_eq!(
        created["context"]["auxData"],
        json!({"isDefault": true, "type": "default"})
    );
    assert!(created["context"]["uniqueId"].is_string());
    let before = page.client.recorder.count();
    page.result("Runtime.enable", json!({}));
    assert_eq!(page.client.recorder.count(), before + 1);
    assert_eq!(page.error("Runtime.nonexistent", json!({}))["code"], -32601);
    page.result("Runtime.runIfWaitingForDebugger", json!({}));
}

#[test]
fn console_calls_arrive_once_with_expandable_arguments_and_unchanged_text() {
    let mut page = Page::new();
    page.enable(None);
    let before = page.client.recorder.count();
    page.evaluate_unpumped(None, "con.log('a', { k: 1 }); con.warn(2)");
    assert_eq!(page.client.recorder.count(), before + 3);
    assert_eq!(
        page.console(2),
        [
            ("log".to_owned(), "a {\"k\":1}".to_owned()),
            ("warn".to_owned(), "2".to_owned())
        ]
    );
    let log = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(log["type"], "log");
    assert_eq!(log["executionContextId"], 1);
    assert!(log["timestamp"].as_f64().unwrap() > 1.0e12);
    assert_eq!(log["args"][0], json!({"type": "string", "value": "a"}));
    assert_eq!(log["args"][1]["preview"]["properties"][0]["name"], "k");
    let expanded = page.properties(&log["args"][1]["objectId"], json!({"ownProperties": true}));
    assert_eq!(expanded["result"][0]["value"]["value"], 1);
    let warn = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(warn["type"], "warning");
    assert_eq!(warn["args"][0]["value"], 2);

    page.result(
        "Runtime.releaseObjectGroup",
        json!({"objectGroup": "console"}),
    );
    assert_eq!(
        page.error(
            "Runtime.getProperties",
            json!({"objectId": log["args"][1]["objectId"]})
        )["code"],
        -32000
    );
}

#[test]
fn a_saved_console_method_arrives_once_as_text() {
    let mut page = Page::new();
    page.enable(None);
    let before = page.client.recorder.count();
    page.evaluate_unpumped(None, "saved('x', 1)");
    assert_eq!(page.client.recorder.count(), before + 2);
    assert_eq!(page.console(1), [("log".to_owned(), "x 1".to_owned())]);
    let logged = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(logged["type"], "log");
    assert_eq!(logged["args"], json!([{"type": "string", "value": "x 1"}]));
}

#[test]
fn a_console_argument_that_logs_while_it_is_printed() {
    let mut page = Page::new();
    page.enable(None);
    let before = page.client.recorder.count();
    page.evaluate_unpumped(
        None,
        "con.log(Object.assign(function () {}, {
           toString() { con.log('inner'); return 'outer'; },
         }))",
    );
    assert_eq!(page.client.recorder.count(), before + 3);
    assert_eq!(
        page.console(2),
        [
            ("log".to_owned(), "inner".to_owned()),
            ("log".to_owned(), "outer".to_owned())
        ]
    );
    let outer = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(outer["args"][0]["type"], "function");
    let inner = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(inner["args"][0]["value"], "inner");
}

#[test]
fn report_error_arrives_as_one_string() {
    let mut page = Page::new();
    page.enable(None);
    page.evaluate_unpumped(
        None,
        "report('careful', { level: 'warning' }); report('broken')",
    );
    assert_eq!(
        page.console(2),
        [
            ("warn".to_owned(), "careful".to_owned()),
            ("error".to_owned(), "broken".to_owned())
        ]
    );
    let warning = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(warning["type"], "warning");
    assert_eq!(
        warning["args"],
        json!([{"type": "string", "value": "careful"}])
    );
    let error = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(error["type"], "error");
}

#[test]
fn after_disable_and_after_the_session_ends_console_calls_reach_only_the_embedder() {
    let mut page = Page::new();
    page.enable(None);
    page.result("Runtime.disable", json!({}));
    let before = page.client.recorder.count();
    page.evaluate_unpumped(None, "con.log('quiet'); saved('saved')");
    assert_eq!(page.client.recorder.count(), before + 1);
    assert_eq!(
        page.console(2),
        [
            ("log".to_owned(), "quiet".to_owned()),
            ("log".to_owned(), "saved".to_owned())
        ]
    );

    page.enable(None);
    let Page { engine, client, .. } = page;
    let recorder = Arc::clone(&client.recorder);
    drop(client);
    let after = recorder.count();
    engine
        .view
        .update_data("{}".to_owned(), String::new())
        .unwrap();
    let mut page = Page {
        client: Client::attach(&engine),
        engine,
        next: Cell::new(1),
    };
    assert_eq!(page.console(1), [("log".to_owned(), "updated".to_owned())]);
    assert_eq!(recorder.count(), after);
    let created = page.enable(None);
    assert!(created["context"]["uniqueId"].is_string());
    page.evaluate_unpumped(None, "con.log('again')");
    let logged = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(logged["args"][0]["value"], "again");
}

#[test]
fn the_bts_session_evaluates_and_captures_its_console() {
    let mut page =
        Page::with(TestViewSpec::new(PAGE).with_background_entry("globalThis.started = true;"));
    let bts = page.bts_session();
    let created = page.enable(Some(&bts));
    assert_eq!(created["context"]["id"], 1);

    let answer = page.evaluate_unpumped(Some(&bts), "typeof console.log === 'function' && 40 + 2");
    assert_eq!(answer["sessionId"], bts.as_str());
    assert_eq!(answer["result"]["result"]["value"], 42, "{answer}");
    let (_, thread) = page
        .client
        .recorder
        .wait_for("the BTS answer", |value| value == &answer);
    assert_eq!(thread.as_deref(), Some("bobcat-workers"));

    page.evaluate_unpumped(Some(&bts), "console.log('bts', { a: 1 })");
    assert_eq!(
        page.console(1),
        [("log".to_owned(), "bts {\"a\":1}".to_owned())]
    );
    let logged = page
        .client
        .event(&mut page.engine, Some(&bts), "Runtime.consoleAPICalled");
    assert_eq!(logged["args"][0]["value"], "bts");
    let properties = page.client.result(
        &mut page.engine,
        Some(&bts),
        "Runtime.getProperties",
        json!({"objectId": logged["args"][1]["objectId"], "ownProperties": true}),
    );
    assert_eq!(properties["result"][0]["name"], "a");
    let thrown = page.client.result(
        &mut page.engine,
        Some(&bts),
        "Runtime.evaluate",
        json!({"expression": "throw new Error('worker')"}),
    );
    assert_eq!(thrown["exceptionDetails"]["text"], "Uncaught");
}

#[test]
fn dropping_the_view_with_held_objects_releases_its_realms() {
    let mut page =
        Page::with(TestViewSpec::new(PAGE).with_background_entry("globalThis.started = true;"));
    let bts = page.bts_session();
    let page_id = page.enable(None)["context"]["uniqueId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bts_id = page.enable(Some(&bts))["context"]["uniqueId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.evaluate("globalThis.kept = { self: null }; kept.self = kept; kept");
    page.evaluate_unpumped(None, "con.log(kept, [kept])");
    page.client.result(
        &mut page.engine,
        Some(&bts),
        "Runtime.evaluate",
        json!({"expression": "({})"}),
    );
    page.evaluate_unpumped(Some(&bts), "console.log({ b: 1 })");
    page.client
        .event(&mut page.engine, Some(&bts), "Runtime.consoleAPICalled");
    assert!(tap_is_live(&page_id) && tap_is_live(&bts_id));

    let Page { engine, client, .. } = page;
    drop(engine);
    let deadline = Instant::now() + PATIENCE;
    while tap_is_live(&page_id) || tap_is_live(&bts_id) {
        assert!(Instant::now() < deadline, "a realm was not released");
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(client);
}

#[test]
fn a_large_typed_array_is_previewed_without_reading_every_element() {
    let mut page = Page::new();
    page.enable(None);
    let typed = page.result(
        "Runtime.evaluate",
        json!({
            "expression": "globalThis.pixels = new Uint8ClampedArray(1000000); pixels[3] = 7; pixels",
            "generatePreview": true,
        }),
    )["result"]
        .clone();
    assert_eq!(typed["description"], "Uint8ClampedArray(1000000)");
    let preview = &typed["preview"];
    assert_eq!(preview["properties"].as_array().unwrap().len(), 100);
    assert_eq!(preview["overflow"], true);
    assert_eq!(
        preview["properties"][3],
        json!({"name": "3", "type": "number", "value": "7"})
    );
    page.evaluate_unpumped(None, "con.log(pixels)");
    let logged = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    let preview = &logged["args"][0]["preview"];
    assert_eq!(preview["properties"].as_array().unwrap().len(), 100);
    assert_eq!(preview["overflow"], true);
    let holes = page.evaluate("const holes = [1, , 3]; holes");
    assert_eq!(
        holes["preview"]["properties"],
        json!([
            {"name": "0", "type": "number", "value": "1"},
            {"name": "2", "type": "number", "value": "3"},
        ])
    );
    assert_eq!(holes["preview"]["overflow"], false);
}

#[test]
fn an_enable_sent_before_boot_captures_console_arguments_after_boot() {
    let mut engine = TestViewSpec::new(PAGE)
        .with_devtools()
        .create(Arc::new(NoWakeup));
    let client = Client::attach(&engine);
    client
        .session
        .send(json!({"id": 1, "method": "Runtime.enable"}).to_string());
    wait_for_boot(&mut engine);
    client.recorder.answer(1);
    let mut page = Page {
        engine,
        client,
        next: Cell::new(2),
    };
    page.client
        .event(&mut page.engine, None, "Runtime.executionContextCreated");
    page.evaluate("1");
    page.evaluate_unpumped(None, "con.log('after', { a: 1 })");
    let logged = page
        .client
        .event(&mut page.engine, None, "Runtime.consoleAPICalled");
    assert_eq!(logged["args"][0]["value"], "after");
    assert!(logged["args"][1]["objectId"].is_string(), "{logged}");
}

const FAILING_WORKER_PAGE: &str = r"
import { Worker } from 'bobcat-internal';
globalThis.renderPage = function () {
  __CreatePage('card', 0);
};
globalThis.broken = new Worker('./broken.js');
";

#[test]
fn a_worker_whose_module_graph_failed_is_enabled_without_installing() {
    let mut page = Page::with(TestViewSpec::new(FAILING_WORKER_PAGE).with_module(
        "app:///broken.js",
        "import 'bobcat:worker'; import './missing.js';",
    ));
    page.result(
        "Target.setAutoAttach",
        json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true}),
    );
    let attached = page
        .client
        .event(&mut page.engine, None, "Target.attachedToTarget");
    assert_eq!(attached["targetInfo"]["url"], "app:///broken.js");
    let worker = attached["sessionId"].as_str().unwrap().to_owned();
    page.enable(Some(&worker));
    let evaluated = page.client.result(
        &mut page.engine,
        Some(&worker),
        "Runtime.evaluate",
        json!({"expression": "40 + 2"}),
    );
    assert_eq!(evaluated["result"]["value"], 42);
}

#[test]
fn errors_the_page_reports_arrive_once_as_exception_thrown() {
    let mut page = Page::new();
    page.enable(None);
    let before = page.client.recorder.count();
    page.evaluate_unpumped(
        None,
        "setTimeout(() => { throw new TypeError('late'); }, 0)",
    );
    let failed = page.engine_event("the timer failure", |event| {
        matches!(event, EngineEvent::TimerFailed(_))
    });
    let EngineEvent::TimerFailed(error) = failed else {
        unreachable!()
    };
    assert!(error.message.contains("TypeError: late"), "{error:?}");
    assert_eq!(page.client.recorder.count(), before + 2);
    let thrown = page
        .client
        .event(&mut page.engine, None, "Runtime.exceptionThrown");
    let details = &thrown["exceptionDetails"];
    assert_eq!(details["text"], "Uncaught");
    assert_eq!(details["exception"]["className"], "TypeError");
    assert!(
        details["exception"]["description"]
            .as_str()
            .unwrap()
            .starts_with("TypeError: late")
    );
    assert!(thrown["timestamp"].as_f64().unwrap() > 1.0e12);

    page.evaluate_unpumped(None, "Promise.reject(new RangeError('left behind')); 1");
    let left = page.engine_event("the leftover rejection", |event| {
        matches!(event, EngineEvent::ScriptRunError(_))
    });
    let EngineEvent::ScriptRunError(error) = left else {
        unreachable!()
    };
    assert!(
        error.message.starts_with("in a devtools request: ")
            && error.message.contains("left behind"),
        "{error:?}"
    );
    let thrown = page
        .client
        .event(&mut page.engine, None, "Runtime.exceptionThrown");
    assert_eq!(
        thrown["exceptionDetails"]["exception"]["className"],
        "RangeError"
    );
}

#[test]
fn errors_the_bts_reports_arrive_once_as_exception_thrown() {
    let mut page =
        Page::with(TestViewSpec::new(PAGE).with_background_entry("globalThis.started = true;"));
    let bts = page.bts_session();
    page.enable(Some(&bts));
    let before = page.client.recorder.count();
    page.evaluate_unpumped(
        Some(&bts),
        "setTimeout(() => { throw new Error('bts late'); }, 0)",
    );
    let threw = page.engine_event("the BTS failure", |event| {
        matches!(event, EngineEvent::WorkerThrew { .. })
    });
    let EngineEvent::WorkerThrew { error, .. } = threw else {
        unreachable!()
    };
    assert!(error.message.contains("bts late"), "{error:?}");
    assert_eq!(page.client.recorder.count(), before + 2);
    let thrown = page
        .client
        .event(&mut page.engine, Some(&bts), "Runtime.exceptionThrown");
    assert!(
        thrown["exceptionDetails"]["exception"]["description"]
            .as_str()
            .unwrap()
            .contains("bts late")
    );

    page.evaluate_unpumped(
        Some(&bts),
        "Promise.reject(new RangeError('bts left behind')); 1",
    );
    let left = page.engine_event("the BTS leftover rejection", |event| {
        matches!(event, EngineEvent::WorkerThrew { error, .. } if error.message.contains("bts left behind"))
    });
    let EngineEvent::WorkerThrew { error, .. } = left else {
        unreachable!()
    };
    assert!(
        error.message.starts_with("in a devtools request: "),
        "{error:?}"
    );
    let thrown = page
        .client
        .event(&mut page.engine, Some(&bts), "Runtime.exceptionThrown");
    assert_eq!(
        thrown["exceptionDetails"]["exception"]["className"],
        "RangeError"
    );
}
