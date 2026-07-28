//! Host-function call benchmarks, `CodSpeed`-compatible.
//!
//! This is the Element PAPI's hot path: a `.web.bundle`'s main-thread script
//! makes one host call per element operation, so a render is thousands of
//! them. Each case installs a host function, compiles a JS driver that calls it
//! `CALLS` times, and then times one invocation of that driver — so the
//! measurement is dominated by the boundary crossing rather than by compilation
//! or realm setup.
//!
//! The shapes mirror the four PAPI members that exist: `__FlushElementTree`
//! (no arguments), `__CreateView` (one number), `__AppendElement` (two numbers
//! returning one), and `__CreatePage` (a string plus a number).

use quickjs_rust_bridge::{EvalOptions, EvalSource, HostFunctionError, HostValue, Realm, Value};

fn main() {
    divan::main();
}

/// Calls per timed iteration. Large enough that per-iteration overhead is
/// noise, small enough to stay well inside the default execution timeout.
const CALLS: usize = 20_000;

/// Builds a realm with `install`ed host functions and a `run` driver that makes
/// `CALLS` calls of `call_expression`.
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

/// The shape every `__Create*` / `__AppendElement` handler has: return a
/// handle and do nothing else, so the benchmark measures the boundary rather
/// than the work behind it.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the signature is dictated by the host-function boundary"
)]
fn number_handler(_: &[HostValue]) -> Result<HostValue, HostFunctionError> {
    Ok(HostValue::Number(1.0))
}

/// `__FlushElementTree()` — no arguments, no return value.
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

/// `__CreateView(parentComponentUniqueID)` — one number in, one number out.
#[divan::bench]
fn one_number_argument(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("create", 1, number_handler)
                .expect("install");
        },
        "create(0)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

/// `__AppendElement(parent, child)` — the most frequent call of all.
#[divan::bench]
fn two_number_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("append", 2, number_handler)
                .expect("install");
        },
        "append(1, 2)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

/// `__CreatePage(componentID, componentCSSID)` — the string-argument path,
/// which has to decode as well as cross.
#[divan::bench]
fn string_and_number_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("page", 2, number_handler)
                .expect("install");
        },
        "page('card', 0)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

/// A handler returning a string, so the return path is measured too.
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
