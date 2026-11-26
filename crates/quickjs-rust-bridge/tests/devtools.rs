//! The `devtools` reflection entries: what they read, and that the reads
//! that must run no JavaScript call no trap, getter or replaced global.

#![cfg(feature = "devtools")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use quickjs_rust_bridge::{
    Context, DescribeLimits, Error, EvalOptions, EvalSource, HostFunctionError, HostValue,
    ObjectClass, OwnKeys, OwnProperty, PromiseState, PropertyKey, PropertySlot, Reflected, Runtime,
    Value, ValueKind,
};

fn runtime_and_realm() -> (Runtime, Context) {
    let runtime = Runtime::new().unwrap();
    let realm = runtime.create_context().unwrap();
    (runtime, realm)
}

fn run(realm: &Context, source: &str) -> Value {
    realm
        .evaluate(EvalSource::new(source), EvalOptions::default())
        .unwrap()
}

fn number(realm: &Context, source: &str) -> f64 {
    run(realm, source).as_number().unwrap()
}

fn string_of(value: &Value) -> String {
    String::from_utf16(&value.to_utf16().unwrap()).unwrap()
}

fn key_text(property: &OwnProperty) -> String {
    match &property.key {
        PropertyKey::String(text) => text.clone(),
        PropertyKey::Symbol { description, .. } => format!("@{description}"),
    }
}

fn read<T>(reflected: Reflected<T>) -> T {
    match reflected {
        Reflected::Read(value) => value,
        Reflected::Proxy => panic!("read a Proxy"),
    }
}

const COUNTING_PROXY: &str = "
    globalThis.trapCalls = 0;
    const throwing = new Proxy({}, {
        get() { globalThis.trapCalls++; throw new Error('trap'); },
    });
    globalThis.target = { inner: 1 };
    globalThis.proxy = new Proxy(target, throwing);
    globalThis.callable = new Proxy(function f() {}, throwing);
    const revocable = Proxy.revocable({}, throwing);
    revocable.revoke();
    globalThis.revoked = revocable.proxy;
";

fn assert_proxies_unread(realm: &Context) {
    for name in ["proxy", "callable", "revoked"] {
        let value = run(realm, name);
        assert!(
            matches!(
                realm.object_class(&value).unwrap(),
                Some(ObjectClass::Proxy)
            ),
            "{name}"
        );
        assert!(matches!(
            realm.own_properties(&value, OwnKeys::All).unwrap(),
            Reflected::Proxy
        ));
        assert!(matches!(
            realm.own_property(&value, "inner").unwrap(),
            Reflected::Proxy
        ));
        assert!(matches!(realm.prototype(&value).unwrap(), Reflected::Proxy));
        realm.describe(&value, DescribeLimits::default()).unwrap();
    }
    assert_eq!(number(realm, "trapCalls"), 0.0);
}

#[test]
fn a_proxy_is_reported_and_its_traps_are_not_called() {
    let (_runtime, realm) = runtime_and_realm();
    run(&realm, COUNTING_PROXY);
    assert_proxies_unread(&realm);
}

#[test]
fn a_proxy_is_recognised_after_page_script_replaced_the_globals() {
    let (_runtime, realm) = runtime_and_realm();
    run(&realm, COUNTING_PROXY);
    run(
        &realm,
        "const fail = () => { globalThis.trapCalls++; throw new Error('replaced'); };
         globalThis.Proxy = fail;
         globalThis.Reflect = { getPrototypeOf: fail, ownKeys: fail };
         Object.getOwnPropertyDescriptor = fail;
         Object.getPrototypeOf = fail;",
    );
    assert_proxies_unread(&realm);
    let plain = run(&realm, "({ a: 1 })");
    assert_eq!(
        read(realm.own_properties(&plain, OwnKeys::All).unwrap()).len(),
        1
    );
    assert_eq!(number(&realm, "trapCalls"), 0.0);
}

#[test]
fn an_accessor_is_listed_and_its_getter_is_not_called() {
    let (_runtime, realm) = runtime_and_realm();
    let object = run(
        &realm,
        "globalThis.getterCalls = 0;
         ({ plain: 1, get loud() { globalThis.getterCalls++; throw new Error('getter'); } })",
    );
    let properties = read(realm.own_properties(&object, OwnKeys::All).unwrap());
    assert_eq!(
        properties.iter().map(key_text).collect::<Vec<_>>(),
        ["plain", "loud"]
    );
    let PropertySlot::Accessor { getter, setter } = &properties[1].slot else {
        panic!("an accessor");
    };
    assert_eq!(getter.as_ref().unwrap().kind(), ValueKind::Function);
    assert!(setter.is_none());
    assert!(properties[1].enumerable && properties[1].configurable);
    let PropertySlot::Data { value, writable } = &properties[0].slot else {
        panic!("a data property");
    };
    assert_eq!(value.as_number(), Some(1.0));
    assert!(writable);
    let one = read(realm.own_property(&object, "loud").unwrap()).unwrap();
    assert!(matches!(one.slot, PropertySlot::Accessor { .. }));
    assert!(read(realm.own_property(&object, "absent").unwrap()).is_none());
    assert_eq!(number(&realm, "getterCalls"), 0.0);
}

#[test]
fn symbol_and_non_enumerable_keys_are_listed() {
    let (_runtime, realm) = runtime_and_realm();
    let object = run(
        &realm,
        "const object = { visible: 1, [Symbol('named')]: 2, [Symbol()]: 3 };
         Object.defineProperty(object, 'hidden', { value: 4 });
         object",
    );
    let properties = read(realm.own_properties(&object, OwnKeys::All).unwrap());
    assert_eq!(
        properties.iter().map(key_text).collect::<Vec<_>>(),
        ["visible", "hidden", "@named", "@"]
    );
    let hidden = &properties[1];
    assert!(!hidden.enumerable && !hidden.configurable);
    assert!(matches!(
        hidden.slot,
        PropertySlot::Data {
            writable: false,
            ..
        }
    ));
    let PropertyKey::Symbol { symbol, .. } = &properties[2].key else {
        panic!("a symbol key");
    };
    assert_eq!(symbol.kind(), ValueKind::Symbol);
    assert_eq!(
        realm.describe(symbol, DescribeLimits::default()).unwrap(),
        "Symbol(named)"
    );
}

#[test]
fn array_elements_and_length_are_own_properties() {
    let (_runtime, realm) = runtime_and_realm();
    let array = run(&realm, "[10, 'b']");
    assert!(matches!(
        realm.object_class(&array).unwrap(),
        Some(ObjectClass::Array)
    ));
    let properties = read(realm.own_properties(&array, OwnKeys::All).unwrap());
    assert_eq!(
        properties.iter().map(key_text).collect::<Vec<_>>(),
        ["0", "1", "length"]
    );
    let PropertySlot::Data { value, .. } = &properties[2].slot else {
        panic!("length is data");
    };
    assert_eq!(value.as_number(), Some(2.0));
    assert!(!properties[2].enumerable);
    let prototype = read(realm.prototype(&array).unwrap()).unwrap();
    assert!(read(realm.own_property(&prototype, "push").unwrap()).is_some());
    let bare = run(&realm, "Object.create(null)");
    assert!(read(realm.prototype(&bare).unwrap()).is_none());
}

#[test]
fn objects_are_classified() {
    let (_runtime, realm) = runtime_and_realm();
    let class = |source: &str| realm.object_class(&run(&realm, source)).unwrap();
    assert!(class("1").is_none());
    assert!(class("'text'").is_none());
    assert!(matches!(
        class("new TypeError('x')"),
        Some(ObjectClass::Error)
    ));
    assert!(matches!(class("(() => {})"), Some(ObjectClass::Function)));
    assert!(matches!(class("new Map()"), Some(ObjectClass::Map)));
    assert!(matches!(class("new Set()"), Some(ObjectClass::Set)));
    assert!(matches!(class("new Date(0)"), Some(ObjectClass::Date)));
    assert!(matches!(class("/a/g"), Some(ObjectClass::RegExp)));
    assert!(matches!(
        class("new (class extends Map {})()"),
        Some(ObjectClass::Map)
    ));
    assert!(matches!(
        class("Object.create({ constructor: { name: 'Map' } })"),
        Some(ObjectClass::Object)
    ));
    assert!(matches!(
        class("new Proxy(new Uint8Array(4), {})"),
        Some(ObjectClass::Proxy)
    ));
    assert!(matches!(
        class("new DataView(new ArrayBuffer(4))"),
        Some(ObjectClass::Object)
    ));
    assert!(matches!(
        class("new Promise(() => {})"),
        Some(ObjectClass::Promise(PromiseState::Pending))
    ));
    let Some(ObjectClass::Promise(PromiseState::Fulfilled(value))) = class("Promise.resolve(7)")
    else {
        panic!("fulfilled");
    };
    assert_eq!(value.as_number(), Some(7.0));
}

#[test]
fn a_rejected_promise_is_read_without_reporting_its_rejection() {
    let (mut runtime, realm) = runtime_and_realm();
    let promise = run(
        &realm,
        "globalThis.rejected = Promise.reject(new Error('no'));
         rejected.catch(() => {});
         rejected",
    );
    let Some(ObjectClass::Promise(PromiseState::Rejected(reason))) =
        realm.object_class(&promise).unwrap()
    else {
        panic!("rejected");
    };
    assert!(matches!(
        realm.object_class(&reason).unwrap(),
        Some(ObjectClass::Error)
    ));
    runtime.drain_pending_jobs(&realm).unwrap();
    assert!(matches!(
        realm.object_class(&promise).unwrap(),
        Some(ObjectClass::Promise(PromiseState::Rejected(_)))
    ));
    runtime.drain_pending_jobs(&realm).unwrap();
}

#[test]
fn values_are_described_in_one_line() {
    let (_runtime, realm) = runtime_and_realm();
    let limits = DescribeLimits {
        depth: 2,
        string_length: 0,
        item_count: 0,
    };
    let describe = |source: &str| realm.describe(&run(&realm, source), limits).unwrap();
    assert_eq!(describe(r#"'say "hi"'"#), r#""say \"hi\"""#);
    assert_eq!(
        describe("({ a: { b: { c: 1 } } })"),
        "{ a: { b: [Object] } }"
    );
    assert_eq!(describe("new Map([['k', 1]])"), r#"Map(1) { "k" => 1 }"#);
    assert_eq!(describe("new Set([1, 2])"), "Set(2) { 1, 2 }");
    assert_eq!(describe("new Date(0)"), "1970-01-01T00:00:00.000Z");
    assert_eq!(describe("/a+b/gi"), "/a+b/gi");
    assert_eq!(describe("Symbol('tag')"), "Symbol(tag)");
    assert_eq!(describe("(function named() {})"), "[Function named]");
    let error = describe(
        "(function thrower() { try { throw new RangeError('bad'); } catch (e) { return e; } })()",
    );
    assert!(error.starts_with("RangeError: bad\n"), "{error}");
    assert!(error.contains("thrower"), "{error}");
    let short = realm
        .describe(
            &run(&realm, "'abcdef'"),
            DescribeLimits {
                string_length: 3,
                ..limits
            },
        )
        .unwrap();
    assert!(short.starts_with("\"abc"), "{short}");
}

#[test]
fn a_value_host_function_hands_over_the_argument_objects() {
    let (_runtime, realm) = runtime_and_realm();
    let kept: Rc<RefCell<Vec<Value>>> = Rc::default();
    let into = Rc::clone(&kept);
    let log = realm
        .function_with_values("log", 1, move |realm, arguments| {
            assert_eq!(
                realm
                    .describe(&arguments[0], DescribeLimits::default())
                    .unwrap(),
                "{ a: 1 }"
            );
            into.borrow_mut().extend_from_slice(arguments);
            Ok(HostValue::Number(f64::from(
                u32::try_from(arguments.len()).unwrap(),
            )))
        })
        .unwrap();
    let global = realm.global_object().unwrap();
    let realm = realm;
    realm.set_property(&global, "log", &log).unwrap();
    assert_eq!(
        number(
            &realm,
            "globalThis.logged = { a: 1 }; log(logged, 'second')"
        ),
        2.0
    );
    run(&realm, "logged.added = 'later'");
    let first = kept.borrow()[0].clone();
    let added = read(realm.own_property(&first, "added").unwrap()).unwrap();
    let PropertySlot::Data { value, .. } = added.slot else {
        panic!("data");
    };
    assert_eq!(string_of(&value), "later");
    assert_eq!(string_of(&kept.borrow()[1]), "second");
    kept.borrow_mut().clear();
}

#[test]
fn a_value_host_function_does_not_keep_its_realm_alive() {
    struct Tracked(Rc<Cell<u32>>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Rc::new(Cell::new(0));
    let (mut runtime, realm) = runtime_and_realm();
    let tracked = Tracked(Rc::clone(&drops));
    let log = realm
        .function_with_values("log", 0, move |_, _| {
            let _ = &tracked;
            Ok(HostValue::Undefined)
        })
        .unwrap();
    let global = realm.global_object().unwrap();
    realm.set_property(&global, "log", &log).unwrap();
    run(&realm, "log({})");
    drop((log, global));
    drop(realm);
    runtime.run_gc();
    assert_eq!(drops.get(), 1, "the realm and its function were freed");
    assert!(format!("{runtime:?}").contains("live_handles: 0"));
}

#[test]
fn json_round_trips_through_the_intrinsics() {
    let (_runtime, realm) = runtime_and_realm();
    run(
        &realm,
        "globalThis.JSON = { stringify() { throw 1; }, parse() { throw 1; } };",
    );
    let parsed = realm.json_parse(r#"{"a":[1,"x",null],"b":true}"#).unwrap();
    assert_eq!(
        realm.json_stringify(&parsed).unwrap().as_deref(),
        Some(r#"{"a":[1,"x",null],"b":true}"#)
    );
    assert_eq!(
        realm.json_stringify(&run(&realm, "undefined")).unwrap(),
        None
    );
    let throwing = run(&realm, "({ toJSON() { throw new TypeError('no JSON'); } })");
    let error = realm.json_stringify(&throwing).unwrap_err();
    assert!(error.message.contains("no JSON"), "{error}");
    assert!(realm.json_parse("{").is_err());
}

#[test]
fn a_value_of_another_realm_is_refused() {
    let (runtime, realm) = runtime_and_realm();
    let other = runtime.create_context().unwrap();
    let foreign = run(&other, "({})");
    assert!(realm.object_class(&foreign).is_err());
    assert!(realm.own_properties(&foreign, OwnKeys::All).is_err());
    assert!(realm.prototype(&foreign).is_err());
    assert!(realm.describe(&foreign, DescribeLimits::default()).is_err());
    assert!(realm.json_stringify(&foreign).is_err());
    assert!(
        realm
            .own_properties(&run(&realm, "1"), OwnKeys::All)
            .is_err()
    );
}

fn non_indexed_keys(realm: &Context, source: &str) -> Vec<String> {
    let object = run(realm, source);
    read(realm.own_properties(&object, OwnKeys::NonIndexed).unwrap())
        .iter()
        .map(key_text)
        .collect()
}

#[test]
fn non_indexed_keys_leave_out_array_indices() {
    let (_runtime, realm) = runtime_and_realm();
    assert_eq!(
        non_indexed_keys(
            &realm,
            "const array = new Array(100000).fill(1);
             array.named = 1;
             array[Symbol('sym')] = 2;
             array",
        ),
        ["length", "named", "@sym"]
    );
    assert_eq!(
        non_indexed_keys(
            &realm,
            "const typed = new Uint8ClampedArray(100000);
             typed.named = 1;
             typed[Symbol('sym')] = 2;
             typed",
        ),
        ["named", "@sym"]
    );
    assert_eq!(non_indexed_keys(&realm, "({ 1: 1, a: 2 })"), ["a"]);
    assert_eq!(
        non_indexed_keys(&realm, "({ 4294967295: 1, '01': 2, 4294967294: 3 })"),
        ["4294967295", "01"]
    );
    assert!(non_indexed_keys(&realm, "[1, 2].slice(2)").contains(&"length".to_owned()));
    let all = run(&realm, "({ 1: 1, a: 2 })");
    assert_eq!(
        read(realm.own_properties(&all, OwnKeys::All).unwrap())
            .iter()
            .map(key_text)
            .collect::<Vec<_>>(),
        ["1", "a"]
    );
}

fn install_forwarding_log(realm: &mut Context, log: &Rc<RefCell<Vec<String>>>) {
    let log = Rc::clone(log);
    let function = realm
        .function_with_values("log", 0, move |realm, arguments| {
            let text = arguments
                .iter()
                .map(|argument| realm.describe(argument, DescribeLimits::default()).unwrap())
                .collect::<Vec<_>>()
                .join(" ");
            log.borrow_mut().push(text);
            let global = realm.global_object().unwrap();
            let original = realm.property(&global, "original").unwrap();
            let mode = realm.property(&global, "mode").unwrap();
            match realm.call(&original, None, arguments) {
                Ok(value) => Ok(HostValue::String(string_of(&value))),
                Err(Error { message, .. }) if string_of(&mode) == "pass" => {
                    Err(HostFunctionError::new(message))
                }
                Err(_) => Ok(HostValue::String("ignored".to_owned())),
            }
        })
        .unwrap();
    let global = realm.global_object().unwrap();
    realm.set_property(&global, "log", &function).unwrap();
}

#[test]
fn a_value_host_function_calls_a_saved_function_with_its_arguments() {
    let (_runtime, mut realm) = runtime_and_realm();
    let log = Rc::default();
    install_forwarding_log(&mut realm, &log);
    let returned = run(
        &realm,
        "globalThis.mode = 'pass';
         globalThis.original = (...items) => {
             globalThis.printed = items.map(String).join('|');
             return 'printed';
         };
         log({ a: 1 }, 'b')",
    );
    assert_eq!(string_of(&returned), "printed");
    assert_eq!(string_of(&run(&realm, "printed")), "[object Object]|b");
    assert_eq!(*log.borrow(), [r#"{ a: 1 } "b""#]);
}

#[test]
fn the_called_function_can_call_the_value_host_function_again() {
    let (_runtime, mut realm) = runtime_and_realm();
    let log = Rc::default();
    install_forwarding_log(&mut realm, &log);
    let returned = run(
        &realm,
        "globalThis.mode = 'pass';
         globalThis.original = (...items) => items.map(String).join('|');
         log({ toString() { return log('inner') + '-outer'; } })",
    );
    assert_eq!(string_of(&returned), "inner-outer");
    assert_eq!(log.borrow().len(), 2);
    assert_eq!(log.borrow()[1], r#""inner""#);
}

#[test]
fn a_throw_from_the_called_function_reaches_the_handler() {
    let (_runtime, mut realm) = runtime_and_realm();
    let log = Rc::default();
    install_forwarding_log(&mut realm, &log);
    run(
        &realm,
        "globalThis.original = () => { throw new TypeError('original failed'); };",
    );
    run(&realm, "globalThis.mode = 'pass'");
    let passed = run(
        &realm,
        "(() => { try { log(1); return 'none'; } catch (e) { return e.message; } })()",
    );
    assert!(string_of(&passed).contains("original failed"));
    run(&realm, "globalThis.mode = 'ignore'");
    assert_eq!(string_of(&run(&realm, "log(2)")), "ignored");
}

#[test]
fn typed_arrays_are_classified_with_their_element_count() {
    let (_runtime, realm) = runtime_and_realm();
    let length = |source: &str| match realm.object_class(&run(&realm, source)).unwrap() {
        Some(ObjectClass::TypedArray { length }) => length,
        other => panic!("{source}: {other:?}"),
    };
    let kinds = [
        "Uint8ClampedArray",
        "Int8Array",
        "Uint8Array",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
        "BigInt64Array",
        "BigUint64Array",
        "Float16Array",
        "Float32Array",
        "Float64Array",
    ];
    for (index, kind) in kinds.iter().enumerate() {
        assert_eq!(
            length(&format!("new {kind}({})", index + 3)),
            index + 3,
            "{kind}"
        );
    }
    assert_eq!(length("new Uint8ClampedArray(8294400)"), 8_294_400);
    assert_eq!(length("new Float32Array(16).subarray(4, 10)"), 6);
    assert_eq!(
        length(
            "const buffer = new ArrayBuffer(16);
             const view = new Int32Array(buffer);
             buffer.transfer();
             view"
        ),
        0
    );
    assert_eq!(number(&realm, "1 + 1"), 2.0);
}
