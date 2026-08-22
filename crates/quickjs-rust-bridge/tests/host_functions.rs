//! Leak, lifetime, and re-entrancy pressure on the host-function boundary.
//!
//! Most tests here drive one of the FFI paths tens of thousands of times
//! inside a realm capped at 2 MB. A reference-count slip in the trampoline —
//! an argument box never freed, a returned value duplicated, an exception path
//! that skips its cleanup — shows up as a `QuickJS` out-of-memory failure
//! rather than as a silently growing process, which is what makes these
//! assertions meaningful.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use quickjs_rust_bridge::{
    CallOutcome, EvalOptions, EvalSource, HostArgument, HostFunctionError, HostValue, Realm,
    RealmOptions,
};

fn tight_realm() -> Realm {
    Realm::with_options(RealmOptions {
        memory_limit: Some(2 * 1024 * 1024),
        ..RealmOptions::default()
    })
    .unwrap()
}

#[test]
fn javascript_shared_memory_is_not_exposed() {
    let mut realm = Realm::new().unwrap();
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
            let HostValue::String(value) = &arguments[0] else {
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
    let mut realm = tight_realm();
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
    let mut realm = tight_realm();
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
                   try { take({ a: 'y'.repeat(100) }); } catch (e) { n++; } \
                 } n",
            ),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(value.as_number(), Some(50000.0));
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
    let mut realm = Realm::new().unwrap();
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
        let mut realm = Realm::new().unwrap();
        realm
            .define_global_function("f", 1, |arguments| {
                Ok(arguments.first().cloned().unwrap_or(HostValue::Null))
            })
            .unwrap();
        let _ = realm.evaluate(EvalSource::new("f('hi')"), EvalOptions::default());
    }
}

#[test]
fn set_property_honors_the_execution_timeout() {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut realm = Realm::with_options(RealmOptions {
            execution_timeout: Some(Duration::from_millis(20)),
            ..RealmOptions::default()
        })
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
    let mut realm = Realm::new().unwrap();
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
    let mut realm = Realm::with_options(RealmOptions {
        memory_limit: Some(300_000),
        ..RealmOptions::default()
    })
    .unwrap();

    let huge = "n".repeat(200_000);
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

    use quickjs_rust_bridge::{EvalOptions, EvalSource, HostFunctionError, HostValue, Realm};

    struct Tracked(Rc<Cell<u32>>);

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn tracking(
        drops: &Rc<Cell<u32>>,
    ) -> impl FnMut(&[HostValue]) -> Result<HostValue, HostFunctionError> + use<> {
        let tracked = Tracked(Rc::clone(drops));
        move |_| {
            let _ = &tracked;
            Ok(HostValue::Undefined)
        }
    }

    #[test]
    fn dropping_an_uninstalled_function_releases_its_closure() {
        let drops = Rc::new(Cell::new(0));
        let mut realm = Realm::new().unwrap();

        let function = realm.function("gone", 0, tracking(&drops)).unwrap();
        assert_eq!(drops.get(), 0, "still referenced");
        drop(function);
        assert_eq!(drops.get(), 0, "finalized, but the drop is deferred");

        realm.run_gc();
        assert_eq!(drops.get(), 1, "reclaimed at the next operation");

        realm.run_gc();
        assert_eq!(drops.get(), 1, "and not released twice");
    }

    #[test]
    fn a_handler_owning_a_value_is_released_without_re_entering_the_collector() {
        let drops = Rc::new(Cell::new(0));
        let mut realm = Realm::new().unwrap();

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
        realm.run_gc();
        assert_eq!(drops.get(), 1, "released outside the collector");

        let value = realm
            .evaluate(EvalSource::new("1 + 1"), EvalOptions::default())
            .expect("the realm is still healthy");
        assert_eq!(value.as_number(), Some(2.0));
    }

    #[test]
    fn replacing_an_installed_global_releases_the_old_closure() {
        let drops = Rc::new(Cell::new(0));
        let mut realm = Realm::new().unwrap();

        realm
            .define_global_function("handler", 0, tracking(&drops))
            .unwrap();
        realm
            .evaluate(
                EvalSource::new("globalThis.handler = 1;"),
                EvalOptions::default(),
            )
            .unwrap();
        realm.run_gc();
        assert_eq!(drops.get(), 1, "the replaced handler must release");
    }

    #[test]
    fn a_reachable_function_is_not_released() {
        let drops = Rc::new(Cell::new(0));
        let mut realm = Realm::new().unwrap();

        realm
            .define_global_function("kept", 0, tracking(&drops))
            .unwrap();
        realm.run_gc();
        assert_eq!(drops.get(), 0, "a live global must survive collection");

        realm
            .evaluate(EvalSource::new("kept()"), EvalOptions::default())
            .expect("a surviving function stays callable");
    }

    #[test]
    fn released_slots_are_reused_rather_than_accumulating() {
        let drops = Rc::new(Cell::new(0));
        let mut realm = Realm::new().unwrap();

        for _ in 0..1000 {
            let function = realm.function("churn", 0, tracking(&drops)).unwrap();
            drop(function);
            realm.run_gc();
        }
        assert_eq!(drops.get(), 1000, "every discarded closure must release");

        realm
            .define_global_function("final", 1, |arguments| {
                Ok(arguments.first().cloned().unwrap_or(HostValue::Null))
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
        let mut realm = Realm::new().unwrap();
        realm
            .define_global_function("rooted", 0, tracking(&drops))
            .unwrap();
        assert_eq!(drops.get(), 0);

        drop(realm);
        assert_eq!(drops.get(), 1, "teardown must release the remainder");
    }
}
