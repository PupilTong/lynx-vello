//! Host-function call benchmarks, `CodSpeed`-compatible.
//!
//! This is the Element PAPI's hot path: a `.web.bundle`'s main-thread script
//! makes one host call per element operation, so a render is thousands of
//! them. Each case installs a host function, compiles a JS driver that calls it
//! `CALLS` times, and then times one invocation of that driver — so the
//! measurement is dominated by the boundary crossing rather than by compilation
//! or realm setup.
//!
//! The shapes mirror four Element PAPI members: `__FlushElementTree` (no
//! arguments), `__CreateView` (one number returning a JS weak ref),
//! `__AppendElement` (two JS weak refs returning the child), and
//! `__CreatePage` (a string plus a number returning a JS weak ref).

use quickjs_rust_bridge::{EvalOptions, EvalSource, HostFunctionError, HostValue, Realm, Value};

fn main() {
    divan::main();
}

const CALLS: usize = 20_000;

fn driver(install: impl FnOnce(&mut Realm), call_expression: &str) -> (Realm, Value, Value) {
    let mut realm = Realm::new().expect("realm");
    realm.set_js_weak_ref_drop(|_| {});
    let global = realm.global_object().expect("global");
    let parent = realm
        .create_weak_ref_with_node_id(u32::MAX - 1)
        .expect("parent weak ref");
    let child = realm
        .create_weak_ref_with_node_id(u32::MAX)
        .expect("child weak ref");
    realm
        .set_property(&global, "parent", &parent)
        .expect("install parent");
    realm
        .set_property(&global, "child", &child)
        .expect("install child");
    install(&mut realm);
    realm
        .evaluate(
            EvalSource::new(&format!(
                "globalThis.run = function run() {{
                   let sink = 0;
                   for (let i = 0; i < {CALLS}; i += 1) {{ {call_expression}; sink += 1; }}
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
fn page_handler(_: &[HostValue]) -> Result<HostValue, HostFunctionError> {
    Ok(HostValue::JsWeakRef(1))
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the signature is dictated by the host-function boundary"
)]
fn append_handler(arguments: &[HostValue]) -> Result<HostValue, HostFunctionError> {
    Ok(arguments[1].clone())
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
fn one_number_argument(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            let mut next_id = 0u32;
            realm
                .define_global_function("create", 1, move |_| {
                    next_id += 1;
                    Ok(HostValue::JsWeakRef(next_id))
                })
                .expect("install");
        },
        "create(0)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

#[divan::bench]
fn two_weak_ref_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("append", 2, append_handler)
                .expect("install");
        },
        "append(parent, child)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

#[divan::bench]
fn string_and_number_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("page", 2, page_handler)
                .expect("install");
        },
        "page('card', 0)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
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
