//! Host-function call benchmarks, `CodSpeed`-compatible.
//!
//! This is the Element PAPI's hot path: a `.web.bundle`'s main-thread script
//! makes one host call per element operation, so a render is thousands of
//! them. Each case installs a host function, compiles a JS driver that calls it
//! `CALLS` times, and then times one invocation of that driver — so the
//! measurement is dominated by the boundary crossing rather than by compilation
//! or realm setup.
//!
//! The four benchmarked call shapes mirror `__FlushElementTree` (no arguments),
//! `__CreateView` (one number returning a host object), `__AppendElement` (two
//! host objects), and `__CreatePage` (a string plus a number returning a host
//! object).

use quickjs_rust_bridge::{
    EvalOptions, EvalSource, HostFunctionError, HostObject, HostValue, Realm, Value,
};

fn main() {
    divan::main();
}

const CALLS: usize = 20_000;

fn driver(install: impl FnOnce(&mut Realm), call_expression: &str) -> (Realm, Value, Value) {
    let mut realm = Realm::new().expect("realm");
    install(&mut realm);
    realm
        .evaluate(
            EvalSource::new(&format!(
                "globalThis.run = function run() {{
                   let sink = 0;
                   for (let i = 0; i < {CALLS}; i += 1) {{ sink += {call_expression}; }}
                   return sink;
                 }};
                 globalThis.run",
            )),
            EvalOptions::default(),
        )
        .expect("compile the driver");
    let run = realm
        .evaluate(EvalSource::new("globalThis.run"), EvalOptions::default())
        .expect("the driver");
    let undefined = realm.undefined().expect("undefined");
    (realm, run, undefined)
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the signature is dictated by the host-function boundary"
)]
fn object_handler(_: &[HostValue]) -> Result<HostValue, HostFunctionError> {
    Ok(HostValue::Object(HostObject::new(1)))
}

#[divan::bench]
fn no_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("flush", 0, |_| Ok(HostValue::Undefined))
                .expect("install");
        },
        "(flush(), 1)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

#[divan::bench]
fn create_object(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("create", 1, object_handler)
                .expect("install");
        },
        "(create(0), 1)",
    );
    let releases = realm.host_object_release_queue();
    bencher.bench_local(|| {
        let result = realm.call(&run, Some(&undefined), &[]).expect("run");
        drop(releases.drain());
        result
    });
}

#[divan::bench]
fn two_object_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("make", 0, object_handler)
                .expect("install");
            realm
                .define_global_function("append", 2, |_| Ok(HostValue::Undefined))
                .expect("install");
            realm
                .evaluate(
                    EvalSource::new("globalThis.parent = make(); globalThis.child = make();"),
                    EvalOptions::default(),
                )
                .expect("create retained handles");
        },
        "(append(parent, child), 1)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

#[divan::bench]
fn string_and_number_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("page", 2, object_handler)
                .expect("install");
        },
        "(page('card', 0), 1)",
    );
    let releases = realm.host_object_release_queue();
    bencher.bench_local(|| {
        let result = realm.call(&run, Some(&undefined), &[]).expect("run");
        drop(releases.drain());
        result
    });
}

#[divan::bench]
fn string_return(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("tag", 1, |_| Ok(HostValue::String("view".to_owned())))
                .expect("install");
        },
        "tag(0).length",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}
