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
    EvalOptions, EvalSource, HostFunctionError, HostValue, Realm, RealmOptions,
};

/// A realm tight enough that a per-call leak cannot survive the loops below.
fn tight_realm() -> Realm {
    Realm::with_options(RealmOptions {
        memory_limit: Some(2 * 1024 * 1024),
        ..RealmOptions::default()
    })
    .unwrap()
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

/// The installed function is rooted by the object it was installed on, not by
/// the Rust `Value` the caller happened to hold.
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

/// Tearing down a realm that still holds host closures must not leak the
/// runtime or double-free the callback table.
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

/// `set_property` can run arbitrary JavaScript — a `Proxy` `set` trap, or an
/// accessor inherited from the prototype chain — so it must execute under the
/// realm's interrupt guard. Without one, a hostile or buggy trap hangs the
/// owner thread forever instead of hitting the configured timeout.
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

/// A trap that runs to completion behaves normally — the guard changes when
/// execution is cut off, not what a successful set does.
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

/// A failed allocation while naming the function must fail the whole
/// construction, not hand back a half-built object.
///
/// `JS_DefinePropertyValue` stores its value without checking for the
/// exception sentinel, so passing it a failed `JS_NewString` would leave a
/// function whose `.name` has no type and a pending exception that surfaces
/// later at an unrelated call.
#[test]
fn a_name_that_cannot_be_allocated_fails_construction_cleanly() {
    let mut realm = Realm::with_options(RealmOptions {
        memory_limit: Some(300_000),
        ..RealmOptions::default()
    })
    .unwrap();

    // Far larger than the remaining budget, so the name string cannot be built.
    let huge = "n".repeat(200_000);
    let outcome = realm.function(&huge, 0, |_| Ok(HostValue::Undefined));
    assert!(outcome.is_err(), "an unallocatable name must not succeed");

    // The realm is still usable and reports types correctly — no sentinel was
    // stored anywhere, and no stale exception is left pending.
    realm
        .define_global_function("ok", 0, |_| Ok(HostValue::Number(1.0)))
        .unwrap();
    let value = realm
        .evaluate(EvalSource::new("typeof ok.name"), EvalOptions::default())
        .expect("the realm is still healthy");
    let units = value.to_utf16().unwrap();
    assert_eq!(String::from_utf16(&units).unwrap(), "string");
}
