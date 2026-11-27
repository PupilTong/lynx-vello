//! Leak, lifetime, and re-entrancy pressure on the host-function boundary.
//!
//! Most tests here drive one of the FFI paths tens of thousands of times
//! inside a realm capped at 2 MB. A reference-count slip in the trampoline —
//! an argument box never freed, a returned value duplicated, an exception path
//! that skips its cleanup — shows up as a `QuickJS` out-of-memory failure
//! rather than as a silently growing process, which is what makes these
//! assertions meaningful.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use quickjs_rust_bridge::{
    CallOutcome, Context, EvalOptions, EvalSource, HostArgument, HostFunctionError, HostValue,
    Runtime, RuntimeOptions, Value,
};

fn realm() -> Context {
    Runtime::new().unwrap().create_context().unwrap()
}

fn tight_realm() -> Context {
    Runtime::with_options(RuntimeOptions {
        memory_limit: Some(2 * 1024 * 1024),
        ..RuntimeOptions::default()
    })
    .unwrap()
    .create_context()
    .unwrap()
}

fn text(realm: &mut Context, source: &str) -> String {
    let value = realm
        .evaluate(EvalSource::new(source), EvalOptions::default())
        .unwrap();
    String::from_utf16(&value.to_utf16().unwrap()).unwrap()
}

#[test]
fn javascript_shared_memory_is_not_exposed() {
    let realm = realm();
    let value = realm
        .evaluate(
            EvalSource::new(
                "(() => {
                    const buffer = new ArrayBuffer(8);
                    const bytes = new Uint8Array(buffer);
                    const view = new DataView(buffer);
                    bytes[0] = 42;
                    view.setUint16(1, 0x1234);
                    return [
                        typeof Atomics,
                        'Atomics' in globalThis,
                        Object.getOwnPropertyDescriptor(globalThis, 'Atomics') === undefined,
                        typeof SharedArrayBuffer,
                        'SharedArrayBuffer' in globalThis,
                        Object.getOwnPropertyDescriptor(globalThis, 'SharedArrayBuffer') === undefined,
                        bytes.buffer === buffer,
                        view.buffer === buffer,
                        ArrayBuffer.isView(bytes),
                        view.getUint8(0),
                        view.getUint16(1),
                    ].join(':');
                })()",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(
        String::from_utf16(&value.to_utf16().unwrap()).unwrap(),
        "undefined:false:true:undefined:false:true:true:true:true:42:4660"
    );
}

#[test]
fn string_round_trip_does_not_leak() {
    let mut realm = tight_realm();
    realm
        .define_global_function("echo", 1, |arguments| {
            let HostArgument::String(value) = arguments[0] else {
                return Err(HostFunctionError::new("expected a string"));
            };
            Ok(HostValue::String(value.repeat(4)))
        })
        .unwrap();
    let value = realm
        .evaluate(
            EvalSource::new(
                "let s = 'x'.repeat(200); let n = 0; \
                 for (let i = 0; i < 50000; i++) { n += echo(s).length; } n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(value.as_number(), Some(50000.0 * 800.0));
}

#[test]
fn repeated_member_calls_do_not_leak() {
    let realm = tight_realm();
    let host = realm
        .evaluate(
            EvalSource::new(
                "globalThis.host = { total: 0 }; \
                 host.take = function take(id, name, detail, last) { \
                   host.total += id + name.length + detail.length + (last ? 1 : 0); \
                 }; \
                 globalThis.host",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    let take = realm.member("take").unwrap();
    let name = "pointermove";
    let detail = "y".repeat(300);
    let mut expected = 0.0f64;
    for index in 0..50_000u32 {
        expected += f64::from(index % 7)
            + f64::from(u32::try_from(name.len() + detail.len()).expect("short test strings"))
            + f64::from(u8::from(index % 2 == 0));
    }
    for index in 0..50_000 {
        let outcome = realm
            .call_member(
                &host,
                &take,
                &[
                    HostArgument::Number(f64::from(index % 7)),
                    HostArgument::String(name),
                    HostArgument::String(&detail),
                    HostArgument::Boolean(index % 2 == 0),
                ],
            )
            .unwrap();
        assert!(matches!(outcome, CallOutcome::Called(_)));
    }
    let total = realm
        .evaluate(EvalSource::new("host.total"), EvalOptions::default())
        .unwrap();
    assert_eq!(total.as_number(), Some(expected));
}

#[test]
fn a_throwing_member_call_does_not_leak() {
    let realm = tight_realm();
    let host = realm
        .evaluate(
            EvalSource::new(
                "globalThis.host = { boom(text) { throw new Error(text); } }; globalThis.host",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    let boom = realm.member("boom").unwrap();
    let text = "z".repeat(300);
    for _ in 0..50_000 {
        realm
            .call_member(&host, &boom, &[HostArgument::String(&text)])
            .expect_err("the member throws every time");
    }
}

#[test]
fn rejected_argument_path_does_not_leak() {
    let mut realm = tight_realm();
    realm
        .define_global_function("take", 1, |_| Ok(HostValue::Undefined))
        .unwrap();
    let value = realm
        .evaluate(
            EvalSource::new(
                "let n = 0; \
                 for (let i = 0; i < 50000; i++) { \
                   try { take({ a: 'y'.repeat(100), f() {} }); } catch (e) { n++; } \
                 } n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(value.as_number(), Some(50000.0));
}

#[test]
fn an_object_comes_back_from_the_host_as_a_deep_copy() {
    let mut realm = realm();
    realm
        .define_global_function("echo", 1, |arguments| {
            Ok(arguments
                .first()
                .map_or(HostValue::Undefined, |argument| argument.to_value()))
        })
        .unwrap();
    assert_eq!(
        text(
            &mut realm,
            "(() => {
                const o = { a: [1, { b: 2 }], u: undefined, n: NaN,
                            d: new Date(1700000000123),
                            bytes: new Uint8Array([1, 2, 255]),
                            big: 9007199254740993n };
                o.self = o;
                const c = echo(o);
                return [
                    c !== o, c.a !== o.a, c.a[0] === 1, c.a[1].b === 2,
                    'u' in c, c.u === undefined, Number.isNaN(c.n),
                    c.d instanceof Date, c.d.getTime() === 1700000000123,
                    c.bytes instanceof Uint8Array, c.bytes.length === 3,
                    c.bytes[2] === 255, c.self === c,
                    typeof c.big === 'bigint', c.big === 9007199254740993n,
                ].join(':');
            })()",
        ),
        [
            "true", "true", "true", "true", "true", "true", "true", "true", "true", "true", "true",
            "true", "true", "true", "true"
        ]
        .join(":")
    );
}

#[test]
fn a_structured_clone_passes_back_into_the_realm_as_a_call_argument() {
    let mut realm = realm();
    let captured: Rc<RefCell<Option<HostValue>>> = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&captured);
    realm
        .define_global_function("keep", 1, move |arguments| {
            *sink.borrow_mut() = arguments.first().map(|argument| argument.to_value());
            Ok(HostValue::Undefined)
        })
        .unwrap();
    let host = realm
        .evaluate(
            EvalSource::new(
                "globalThis.host = { seen: null, take(value) { host.seen = value; } }; \
                 const o = { tag: 'kept', list: [1, 2, 3] }; o.self = o; keep(o); \
                 globalThis.host",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    let kept = captured.borrow_mut().take().expect("the host kept it");
    assert!(matches!(kept, HostValue::Structured(_)));

    let take = realm.member("take").unwrap();
    let outcome = realm
        .call_member(&host, &take, &[kept.as_argument()])
        .unwrap();
    assert!(matches!(outcome, CallOutcome::Called(_)));
    assert_eq!(
        text(
            &mut realm,
            "[host.seen.tag, host.seen.list.join(','), host.seen.self === host.seen].join(':')",
        ),
        "kept:1,2,3:true"
    );
}

#[test]
fn a_structured_clone_written_in_one_runtime_reads_in_another() {
    let captured: Rc<RefCell<Option<HostValue>>> = Rc::new(RefCell::new(None));
    let mut writer = realm();
    let sink = Rc::clone(&captured);
    writer
        .define_global_function("keep", 1, move |arguments| {
            *sink.borrow_mut() = arguments.first().map(|argument| argument.to_value());
            Ok(HostValue::Undefined)
        })
        .unwrap();
    writer
        .evaluate(
            EvalSource::new(
                "const o = { tag: 'from A', bytes: new Uint8Array([7, 8]) }; o.self = o; keep(o);",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    let kept = captured.borrow_mut().take().expect("the host kept it");

    let mut reader = realm();
    let host = reader
        .evaluate(
            EvalSource::new(
                "globalThis.host = { seen: null, take(value) { host.seen = value; } }; \
                 globalThis.host",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    let take = reader.member("take").unwrap();
    reader
        .call_member(&host, &take, &[kept.as_argument()])
        .unwrap();
    assert_eq!(
        text(
            &mut reader,
            "[host.seen.tag, host.seen.bytes instanceof Uint8Array, \
             host.seen.bytes.join(','), host.seen.self === host.seen].join(':')",
        ),
        "from A:true:7,8:true"
    );
}

#[test]
fn a_function_argument_throws_a_type_error_and_never_reaches_the_host() {
    let mut realm = realm();
    let calls = Rc::new(Cell::new(0u32));
    let counter = Rc::clone(&calls);
    realm
        .define_global_function("take", 1, move |_| {
            counter.set(counter.get() + 1);
            Ok(HostValue::Undefined)
        })
        .unwrap();
    assert_eq!(
        text(
            &mut realm,
            "(() => {
                const thrown = [];
                for (const value of [function f() {}, { nested: () => 1 }]) {
                    try { take(value); thrown.push('no throw'); }
                    catch (e) { thrown.push(e instanceof TypeError ? 'TypeError' : String(e)); }
                }
                return thrown.join(':');
            })()",
        ),
        "TypeError:TypeError"
    );
    assert_eq!(
        calls.get(),
        0,
        "the serializer refuses before the host is dispatched to"
    );
    assert_eq!(text(&mut realm, "String(take({ ok: 1 }))"), "undefined");
    assert_eq!(calls.get(), 1);
}

#[test]
fn structured_round_trip_does_not_leak() {
    let mut realm = tight_realm();
    realm
        .define_global_function("echo", 1, |arguments| {
            Ok(arguments
                .first()
                .map_or(HostValue::Undefined, |argument| argument.to_value()))
        })
        .unwrap();
    let value = realm
        .evaluate(
            EvalSource::new(
                "const o = { text: 'x'.repeat(200), list: [1, 2, 3, 4, 5, 6, 7, 8], \
                   d: new Date(0), bytes: new Uint8Array(64) }; \
                 o.self = o; \
                 let n = 0; for (let i = 0; i < 20000; i++) { n += echo(o).list.length; } n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(value.as_number(), Some(20000.0 * 8.0));
}

#[test]
fn throwing_host_function_does_not_leak() {
    let mut realm = tight_realm();
    realm
        .define_global_function("boom", 1, |_| Err(HostFunctionError::new("nope")))
        .unwrap();
    let value = realm
        .evaluate(
            EvalSource::new(
                "let n = 0; let s = 'z'.repeat(300); \
                 for (let i = 0; i < 50000; i++) { try { boom(s); } catch (e) { n++; } } n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(value.as_number(), Some(50000.0));
}

#[test]
fn many_arguments_do_not_leak() {
    let mut realm = tight_realm();
    realm
        .define_global_function("count", 0, |arguments| {
            #[allow(
                clippy::cast_precision_loss,
                reason = "an argument count is far below f64's exact-integer range"
            )]
            Ok(HostValue::Number(arguments.len() as f64))
        })
        .unwrap();
    let value = realm
        .evaluate(
            EvalSource::new(
                "let s = 'q'.repeat(100); let n = 0; \
                 for (let i = 0; i < 20000; i++) { \
                   n += count(s, s+i, s+'a', s+'b', s+'c', s+'d', s+'e', s+'f'); \
                 } n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(value.as_number(), Some(20000.0 * 8.0));
}

#[test]
fn a_dropped_function_value_stays_callable_once_installed() {
    let realm = realm();
    let global = realm.global_object().unwrap();
    let function = realm
        .function("f", 0, |_| Ok(HostValue::Number(5.0)))
        .unwrap();
    realm.set_property(&global, "f", &function).unwrap();
    drop(function);
    drop(global);

    let value = realm
        .evaluate(EvalSource::new("f()"), EvalOptions::default())
        .unwrap();
    assert_eq!(value.as_number(), Some(5.0));
}

#[test]
fn a_realm_with_live_host_functions_drops_cleanly() {
    for _ in 0..50 {
        let mut realm = realm();
        realm
            .define_global_function("f", 1, |arguments| {
                Ok(arguments
                    .first()
                    .map_or(HostValue::Null, |argument| argument.to_value()))
            })
            .unwrap();
        let _ = realm.evaluate(EvalSource::new("f('hi')"), EvalOptions::default());
    }
}

#[test]
fn set_property_honors_the_execution_timeout() {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let realm = Runtime::with_options(RuntimeOptions {
            execution_timeout: Some(Duration::from_millis(20)),
            ..RuntimeOptions::default()
        })
        .unwrap()
        .create_context()
        .unwrap();
        let target = realm
            .evaluate(
                EvalSource::new("new Proxy({}, { set() { for (;;) {} } })"),
                EvalOptions::default(),
            )
            .unwrap();
        let value = realm.number(1.0).unwrap();
        let _ = sender.send(realm.set_property(&target, "x", &value).is_err());
    });

    let errored = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("set_property hung past the configured execution timeout");
    assert!(errored, "a timed-out trap must report an error");
}

#[test]
fn set_property_runs_a_proxy_trap_and_reports_refusal() {
    let realm = realm();
    let accepting = realm
        .evaluate(
            EvalSource::new(
                "globalThis.hits = 0; new Proxy({}, { set() { globalThis.hits++; return true; } })",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    let value = realm.number(1.0).unwrap();
    realm.set_property(&accepting, "x", &value).unwrap();
    assert_eq!(
        realm
            .evaluate(EvalSource::new("hits"), EvalOptions::default())
            .unwrap()
            .as_number(),
        Some(1.0)
    );

    let refusing = realm
        .evaluate(
            EvalSource::new("new Proxy({}, { set() { return false; } })"),
            EvalOptions::default(),
        )
        .unwrap();
    assert!(
        realm.set_property(&refusing, "x", &value).is_err(),
        "a refusing trap must not report success"
    );
}

#[test]
fn a_name_that_cannot_be_allocated_fails_construction_cleanly() {
    const MEMORY_LIMIT: usize = 300_000;
    let mut realm = Runtime::with_options(RuntimeOptions {
        memory_limit: Some(MEMORY_LIMIT),
        ..RuntimeOptions::default()
    })
    .unwrap()
    .create_context()
    .unwrap();

    let huge = "n".repeat(MEMORY_LIMIT + 1);
    let outcome = realm.function(&huge, 0, |_| Ok(HostValue::Undefined));
    assert!(outcome.is_err(), "an unallocatable name must not succeed");

    realm
        .define_global_function("ok", 0, |_| Ok(HostValue::Number(1.0)))
        .unwrap();
    let value = realm
        .evaluate(EvalSource::new("typeof ok.name"), EvalOptions::default())
        .expect("the realm is still healthy");
    let units = value.to_utf16().unwrap();
    assert_eq!(String::from_utf16(&units).unwrap(), "string");
}

mod release {
    use std::cell::Cell;
    use std::rc::Rc;

    use quickjs_rust_bridge::{
        Context, EvalOptions, EvalSource, HostArgument, HostFunctionError, HostValue, Runtime,
    };

    fn runtime_and_realm() -> (Runtime, Context) {
        let runtime = Runtime::new().unwrap();
        let realm = runtime.create_context().unwrap();
        (runtime, realm)
    }

    struct Tracked(Rc<Cell<u32>>);

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn tracking(
        drops: &Rc<Cell<u32>>,
    ) -> impl FnMut(&[HostArgument<'_>]) -> Result<HostValue, HostFunctionError> + use<> {
        let tracked = Tracked(Rc::clone(drops));
        move |_| {
            let _ = &tracked;
            Ok(HostValue::Undefined)
        }
    }

    #[test]
    fn dropping_an_uninstalled_function_releases_its_closure() {
        let drops = Rc::new(Cell::new(0));
        let (mut runtime, realm) = runtime_and_realm();

        let function = realm.function("gone", 0, tracking(&drops)).unwrap();
        assert_eq!(drops.get(), 0, "still referenced");
        drop(function);
        assert_eq!(drops.get(), 0, "finalized, but the drop is deferred");

        runtime.run_gc();
        assert_eq!(drops.get(), 1, "reclaimed at the next operation");

        runtime.run_gc();
        assert_eq!(drops.get(), 1, "and not released twice");
    }

    #[test]
    fn a_handler_owning_a_value_is_released_without_re_entering_the_collector() {
        let drops = Rc::new(Cell::new(0));
        let (mut runtime, realm) = runtime_and_realm();

        let rooted = realm
            .evaluate(EvalSource::new("({})"), EvalOptions::default())
            .unwrap();
        let tracked = Tracked(Rc::clone(&drops));
        let function = realm
            .function("owns_a_value", 0, move |_| {
                let _ = &tracked;
                let _ = &rooted;
                Ok(HostValue::Undefined)
            })
            .unwrap();

        drop(function);
        runtime.run_gc();
        assert_eq!(drops.get(), 1, "released outside the collector");

        let value = realm
            .evaluate(EvalSource::new("1 + 1"), EvalOptions::default())
            .expect("the realm is still healthy");
        assert_eq!(value.as_number(), Some(2.0));
    }

    #[test]
    fn replacing_an_installed_global_releases_the_old_closure() {
        let drops = Rc::new(Cell::new(0));
        let (mut runtime, mut realm) = runtime_and_realm();

        realm
            .define_global_function("handler", 0, tracking(&drops))
            .unwrap();
        realm
            .evaluate(
                EvalSource::new("globalThis.handler = 1;"),
                EvalOptions::default(),
            )
            .unwrap();
        runtime.run_gc();
        assert_eq!(drops.get(), 1, "the replaced handler must release");
    }

    #[test]
    fn a_reachable_function_is_not_released() {
        let drops = Rc::new(Cell::new(0));
        let (mut runtime, mut realm) = runtime_and_realm();

        realm
            .define_global_function("kept", 0, tracking(&drops))
            .unwrap();
        runtime.run_gc();
        assert_eq!(drops.get(), 0, "a live global must survive collection");

        realm
            .evaluate(EvalSource::new("kept()"), EvalOptions::default())
            .expect("a surviving function stays callable");
    }

    #[test]
    fn released_slots_are_reused_rather_than_accumulating() {
        let drops = Rc::new(Cell::new(0));
        let (mut runtime, mut realm) = runtime_and_realm();

        for _ in 0..1000 {
            let function = realm.function("churn", 0, tracking(&drops)).unwrap();
            drop(function);
            runtime.run_gc();
        }
        assert_eq!(drops.get(), 1000, "every discarded closure must release");

        realm
            .define_global_function("final", 1, |arguments| {
                Ok(arguments
                    .first()
                    .map_or(HostValue::Null, |argument| argument.to_value()))
            })
            .unwrap();
        let value = realm
            .evaluate(EvalSource::new("final(7)"), EvalOptions::default())
            .unwrap();
        assert_eq!(value.as_number(), Some(7.0));
    }

    #[test]
    fn realm_teardown_releases_still_rooted_closures() {
        let drops = Rc::new(Cell::new(0));
        let (_runtime, mut realm) = runtime_and_realm();
        realm
            .define_global_function("rooted", 0, tracking(&drops))
            .unwrap();
        assert_eq!(drops.get(), 0);

        drop(realm);
        assert_eq!(drops.get(), 1, "teardown must release the remainder");
    }
}

fn define_reentrant(
    realm: &mut Context,
    name: &str,
    arity: u32,
    handler: impl Fn(&[HostArgument<'_>]) -> Result<HostValue, HostFunctionError> + 'static,
) {
    let function = realm.function_reentrant(name, arity, handler).unwrap();
    let global = realm.global_object().unwrap();
    realm.set_property(&global, name, &function).unwrap();
}

type Reentry = Rc<OnceCell<Weak<Context>>>;

fn share(realm: Context, reentry: &Reentry) -> Rc<Context> {
    let realm = Rc::new(realm);
    assert!(reentry.set(Rc::downgrade(&realm)).is_ok());
    realm
}

fn evaluate_again(reentry: &Reentry, source: &str) -> Result<Value, HostFunctionError> {
    let realm = reentry
        .get()
        .and_then(Weak::upgrade)
        .expect("the realm is running this call");
    realm
        .evaluate(EvalSource::new(source), EvalOptions::default())
        .map_err(|error| HostFunctionError::new(error.message))
}

fn shared_text(realm: &Context, source: &str) -> String {
    let value = realm
        .evaluate(EvalSource::new(source), EvalOptions::default())
        .unwrap();
    String::from_utf16(&value.to_utf16().unwrap()).unwrap()
}

#[test]
fn a_reentrant_host_function_runs_again_inside_its_own_call() {
    let mut realm = realm();
    let reentry = Reentry::default();
    let again = Rc::clone(&reentry);
    define_reentrant(&mut realm, "nest", 1, move |arguments| {
        let HostArgument::Number(depth) = arguments[0] else {
            return Err(HostFunctionError::new("expected a depth"));
        };
        if depth == 0.0 {
            return Ok(HostValue::String("0".repeat(64)));
        }
        let inner = evaluate_again(&again, &format!("nest({})", depth - 1.0))?;
        let inner = String::from_utf16(&inner.to_utf16().unwrap()).unwrap();
        Ok(HostValue::String(format!("{depth}[{inner}]")))
    });
    let realm = share(realm, &reentry);
    assert_eq!(
        shared_text(&realm, "nest(3)"),
        format!("3[2[1[{}]]]", "0".repeat(64))
    );
}

#[test]
fn nested_host_calls_each_return_their_own_structured_clone() {
    let mut realm = realm();
    let reentry = Reentry::default();
    let again = Rc::clone(&reentry);
    define_reentrant(&mut realm, "echo", 2, move |arguments| {
        let HostArgument::Number(depth) = arguments[1] else {
            return Err(HostFunctionError::new("expected a depth"));
        };
        if depth > 0.0 {
            let inner = evaluate_again(
                &again,
                &format!(
                    "(() => {{ const d = {}; const o = echo({{ depth: d, text: 'y'.repeat(100 + d) }}, d); \
                       if (o.depth !== d || o.text.length !== 100 + d) throw Error('inner ' + JSON.stringify(o)); \
                       return o.depth; }})()",
                    depth - 1.0
                ),
            )?;
            assert_eq!(inner.as_number(), Some(depth - 1.0));
        }
        Ok(arguments[0].to_value())
    });
    let realm = share(realm, &reentry);
    assert_eq!(
        shared_text(
            &realm,
            "(() => { const o = echo({ depth: 4, text: 'z'.repeat(10) }, 4); \
               return o.depth + ':' + o.text; })()",
        ),
        format!("4:{}", "z".repeat(10))
    );
}

#[test]
fn an_exclusive_host_function_refuses_a_nested_call() {
    let mut realm = realm();
    let reentry = Reentry::default();
    let again = Rc::clone(&reentry);
    realm
        .define_global_function("nest", 1, move |arguments| {
            let HostArgument::Number(depth) = arguments[0] else {
                return Err(HostFunctionError::new("expected a depth"));
            };
            if depth == 0.0 {
                return Ok(HostValue::String("reached".to_owned()));
            }
            let refused =
                evaluate_again(&again, "nest(0)").expect_err("the nested call is refused");
            Ok(HostValue::String(refused.to_string()))
        })
        .unwrap();
    let realm = share(realm, &reentry);
    assert!(
        shared_text(&realm, "nest(1)").contains("cannot be called while it is already running"),
        "the nested call threw the refusal"
    );
}

type SeenBytes = Rc<RefCell<Vec<Option<Vec<u8>>>>>;

fn define_take(realm: &mut Context, seen: &SeenBytes) {
    let sink = Rc::clone(seen);
    realm
        .define_global_function("take", 3, move |arguments| {
            let bytes = match arguments.first() {
                Some(HostArgument::Bytes(bytes)) => Some(bytes.to_vec()),
                _ => None,
            };
            #[allow(clippy::cast_precision_loss, reason = "test buffers are small")]
            let answer = bytes.as_ref().map_or(-1.0, |bytes| bytes.len() as f64);
            sink.borrow_mut().push(bytes);
            Ok(HostValue::Number(answer))
        })
        .unwrap();
}

#[test]
fn a_bare_array_buffer_argument_arrives_as_its_bytes() {
    let mut realm = realm();
    let seen = SeenBytes::default();
    define_take(&mut realm, &seen);
    assert_eq!(
        text(
            &mut realm,
            "[take(new Uint8Array([1, 2, 255]).buffer), take(new ArrayBuffer(0))].join(':')"
        ),
        "3:0"
    );
    assert_eq!(
        *seen.borrow(),
        [Some(vec![1, 2, 255]), Some(Vec::new())],
        "contents and length, an empty buffer included"
    );
}

#[test]
fn a_view_is_passed_as_its_buffer_with_its_offset_and_length() {
    let mut realm = realm();
    let window: Rc<RefCell<Option<Vec<u8>>>> = Rc::default();
    let sink = Rc::clone(&window);
    realm
        .define_global_function("window", 3, move |arguments| {
            let [
                HostArgument::Bytes(bytes),
                HostArgument::Number(offset),
                HostArgument::Number(length),
            ] = arguments
            else {
                return Err(HostFunctionError::new("expected (buffer, offset, length)"));
            };
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the test passes small whole offsets"
            )]
            let (offset, length) = (*offset as usize, *length as usize);
            *sink.borrow_mut() = Some(bytes[offset..offset + length].to_vec());
            Ok(HostValue::Undefined)
        })
        .unwrap();
    realm
        .evaluate(
            EvalSource::new(
                "const all = new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7]); \
                 const view = new Uint8Array(all.buffer, 2, 3); \
                 window(view.buffer, view.byteOffset, view.byteLength);",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(window.borrow_mut().take(), Some(vec![2, 3, 4]));
}

#[test]
fn returned_bytes_become_an_array_buffer() {
    let mut realm = realm();
    realm
        .define_global_function("make", 1, |arguments| {
            let Some(&HostArgument::Number(length)) = arguments.first() else {
                return Err(HostFunctionError::new("expected a length"));
            };
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the test asks for small whole lengths"
            )]
            let bytes: Vec<u8> = (0..length as usize).map(|i| (i % 251) as u8).collect();
            Ok(HostValue::Bytes(bytes.into_boxed_slice()))
        })
        .unwrap();
    assert_eq!(
        text(
            &mut realm,
            "(() => {
                const b = make(300);
                const u = new Uint8Array(b);
                const empty = make(0);
                return [
                    b instanceof ArrayBuffer, b.byteLength, u[0], u[1], u[299],
                    b.resizable, empty instanceof ArrayBuffer, empty.byteLength,
                ].join(':');
            })()"
        ),
        "true:300:0:1:48:false:true:0"
    );
}

#[test]
fn a_returned_buffer_can_be_written_transferred_and_detached() {
    let mut realm = realm();
    realm
        .define_global_function("make", 0, |_| {
            Ok(HostValue::Bytes(Box::from([10u8, 20, 30, 40])))
        })
        .unwrap();
    assert_eq!(
        text(
            &mut realm,
            "(() => {
                const out = [];
                const a = make();
                new Uint8Array(a)[0] = 11;
                try { a.resize(2); out.push('resized'); }
                catch (e) { out.push(e instanceof TypeError ? 'TypeError' : String(e)); }
                const same = a.transfer();
                out.push(a.detached, a.byteLength, new Uint8Array(same).join(','));
                const longer = same.transfer(6);
                out.push(same.detached, new Uint8Array(longer).join(','));
                const b = make();
                const shorter = b.transfer(2);
                out.push(new Uint8Array(shorter).join(','));
                const c = make();
                const empty = c.transfer(0);
                out.push(c.detached, empty.byteLength);
                return out.join(':');
            })()"
        ),
        "TypeError:true:0:11,20,30,40:true:11,20,30,40,0,0:10,20:true:0"
    );
}

#[test]
fn views_nested_and_resizable_buffers_keep_the_structured_path() {
    let mut realm = realm();
    let kinds: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    let sink = Rc::clone(&kinds);
    realm
        .define_global_function("echo", 1, move |arguments| {
            let argument = arguments[0];
            sink.borrow_mut().push(match argument {
                HostArgument::Structured(_) => "structured",
                HostArgument::Bytes(_) => "bytes",
                _ => "other",
            });
            Ok(argument.to_value())
        })
        .unwrap();
    assert_eq!(
        text(
            &mut realm,
            "(() => {
                const u = echo(new Uint8Array([1, 2, 3]));
                let d;
                try { echo(new DataView(new ArrayBuffer(2))); d = 'no throw'; }
                catch (e) { d = e instanceof TypeError ? 'TypeError' : String(e); }
                const n = echo({ b: new Uint8Array([6, 7]).buffer });
                const r = new ArrayBuffer(2, { maxByteLength: 8 });
                new Uint8Array(r).set([8, 9]);
                const rr = echo(r);
                const bare = echo(new Uint8Array([10]).buffer);
                return [
                    u instanceof Uint8Array, u.join(','),
                    d,
                    n.b instanceof ArrayBuffer, new Uint8Array(n.b).join(','),
                    rr instanceof ArrayBuffer, rr.resizable, rr.maxByteLength,
                    new Uint8Array(rr).join(','),
                    bare instanceof ArrayBuffer, bare.resizable, new Uint8Array(bare).join(','),
                ].join(':');
            })()"
        ),
        "true:1,2,3:TypeError:true:6,7:true:true:8:8,9:true:false:10"
    );
    assert_eq!(
        *kinds.borrow(),
        ["structured", "structured", "structured", "bytes"]
    );
}

#[test]
fn an_array_buffer_subclass_instance_crosses_as_bytes() {
    let mut realm = realm();
    let seen = SeenBytes::default();
    define_take(&mut realm, &seen);
    assert_eq!(
        text(
            &mut realm,
            "class Sub extends ArrayBuffer {} \
             String(take(new Sub(2)))"
        ),
        "2"
    );
    assert_eq!(*seen.borrow(), [Some(vec![0, 0])]);
}

#[test]
fn describing_a_buffer_runs_no_getter_of_the_realm() {
    let mut realm = realm();
    let seen = SeenBytes::default();
    define_take(&mut realm, &seen);
    assert_eq!(
        text(
            &mut realm,
            "let runs = 0; \
             Object.defineProperty(ArrayBuffer.prototype, 'resizable', \
               { get() { runs++; return true; } }); \
             [take(new ArrayBuffer(3)), runs].join(':')"
        ),
        "3:0"
    );
    assert_eq!(*seen.borrow(), [Some(vec![0, 0, 0])]);
}

#[test]
fn a_detached_buffer_is_a_type_error_at_the_call() {
    let mut realm = realm();
    let seen = SeenBytes::default();
    define_take(&mut realm, &seen);
    assert_eq!(
        text(
            &mut realm,
            "(() => {
                const out = [];
                const fixed = new ArrayBuffer(4); fixed.transfer();
                const resizable = new ArrayBuffer(4, { maxByteLength: 8 }); resizable.transfer();
                for (const value of [fixed, resizable, { inner: fixed }]) {
                    try { take(value); out.push('no throw'); }
                    catch (e) { out.push(e instanceof TypeError ? 'TypeError' : String(e)); }
                }
                return out.join(':');
            })()"
        ),
        "TypeError:TypeError:TypeError"
    );
    assert!(seen.borrow().is_empty(), "the host function never ran");
    assert_eq!(text(&mut realm, "String(take(new ArrayBuffer(1)))"), "1");
}

#[test]
fn bytes_held_across_a_call_into_the_realm_are_unchanged() {
    for reentrant in [false, true] {
        let mut realm = realm();
        let reentry = Reentry::default();
        let again = Rc::clone(&reentry);
        let handler = move |arguments: &[HostArgument<'_>]| {
            let HostArgument::Bytes(bytes) = arguments[0] else {
                return Err(HostFunctionError::new("expected bytes"));
            };
            let before = bytes.to_vec();
            evaluate_again(
                &again,
                "new Uint8Array(globalThis.held).fill(9); \
                 globalThis.held = globalThis.held.transfer(4096); \
                 globalThis.held.transfer();",
            )?;
            evaluate_again(&again, "globalThis.other = new Uint8Array(64).fill(7);")?;
            assert_eq!(bytes, before.as_slice(), "the lent bytes did not change");
            Ok(HostValue::Bytes(Box::from(bytes)))
        };
        if reentrant {
            define_reentrant(&mut realm, "hold", 1, handler);
        } else {
            realm.define_global_function("hold", 1, handler).unwrap();
        }
        let realm = share(realm, &reentry);
        assert_eq!(
            shared_text(
                &realm,
                "globalThis.held = new Uint8Array(64).map((_, i) => i).buffer; \
                 const back = new Uint8Array(hold(globalThis.held)); \
                 [back.length, back[0], back[63], globalThis.held.detached].join(':')"
            ),
            "64:0:63:true",
            "reentrant: {reentrant}"
        );
    }
}

#[test]
fn kept_bytes_arrive_in_another_realm_as_an_array_buffer() {
    let captured: Rc<RefCell<Option<HostValue>>> = Rc::default();
    let mut writer = realm();
    let sink = Rc::clone(&captured);
    writer
        .define_global_function("keep", 1, move |arguments| {
            *sink.borrow_mut() = arguments.first().map(|argument| argument.to_value());
            Ok(HostValue::Undefined)
        })
        .unwrap();
    writer
        .evaluate(
            EvalSource::new("keep(new Uint8Array([7, 8, 9]).buffer)"),
            EvalOptions::default(),
        )
        .unwrap();
    let kept = captured.borrow_mut().take().expect("the host kept it");
    assert_eq!(kept, HostValue::Bytes(Box::from([7u8, 8, 9])));

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = realm();
        let host = reader
            .evaluate(
                EvalSource::new(
                    "globalThis.host = { seen: null, take(value) { host.seen = value; } }; \
                     globalThis.host",
                ),
                EvalOptions::default(),
            )
            .unwrap();
        let take = reader.member("take").unwrap();
        reader
            .call_member(&host, &take, &[kept.as_argument()])
            .unwrap();
        sender
            .send(text(
                &mut reader,
                "[host.seen instanceof ArrayBuffer, host.seen.resizable, \
                 new Uint8Array(host.seen).join(',')].join(':')",
            ))
            .unwrap();
    })
    .join()
    .unwrap();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        "true:false:7,8,9"
    );
}

#[test]
fn empty_bytes_lent_into_a_call_make_an_empty_buffer() {
    let mut realm = realm();
    let host = realm
        .evaluate(
            EvalSource::new(
                "globalThis.host = { seen: null, take(value) { host.seen = value; } }; \
                 globalThis.host",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    let take = realm.member("take").unwrap();
    realm
        .call_member(&host, &take, &[HostArgument::Bytes(&[])])
        .unwrap();
    assert_eq!(
        text(
            &mut realm,
            "[host.seen instanceof ArrayBuffer, host.seen.byteLength].join(':')"
        ),
        "true:0"
    );
}

#[test]
fn bytes_arguments_do_not_leak() {
    let mut realm = tight_realm();
    realm
        .define_global_function("take", 1, |arguments| {
            let Some(HostArgument::Bytes(bytes)) = arguments.first() else {
                return Err(HostFunctionError::new("expected bytes"));
            };
            Ok(HostValue::Number(f64::from(bytes[0])))
        })
        .unwrap();
    let value = realm
        .evaluate(
            EvalSource::new(
                "const b = new Uint8Array(64 * 1024).fill(3).buffer; \
                 let n = 0; for (let i = 0; i < 20000; i++) { n += take(b); } n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(value.as_number(), Some(60000.0));
}
