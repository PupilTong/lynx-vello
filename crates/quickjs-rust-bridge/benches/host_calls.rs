//! Host-function call benchmarks, `CodSpeed`-compatible.
//!
//! This is the Element PAPI's hot path: a `.web.bundle`'s main-thread script
//! makes one host call per element operation, so a render is thousands of
//! them. Each case installs a host function, compiles a JS driver that calls it
//! `CALLS` times, and then times one invocation of that driver — so the
//! measurement is dominated by the boundary crossing rather than by compilation
//! or realm setup.
//!
//! The shapes mirror four `bobcat` members: `flushElementTree` (no
//! arguments), `createElement` (a string plus a number), `insertBefore` (two
//! numbers), and a string-returning query. Element identity crosses the
//! boundary as plain numbers; handle objects never leave JavaScript.

use quickjs_rust_bridge::{
    Context, EvalOptions, EvalSource, HostArgument, HostFunctionError, HostValue, Runtime, Value,
};

fn main() {
    divan::main();
}

const CALLS: usize = 20_000;

fn driver(install: impl FnOnce(&mut Context), call_expression: &str) -> (Context, Value, Value) {
    let mut realm = Runtime::new()
        .expect("runtime")
        .create_context()
        .expect("realm");
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
                    Ok(HostValue::Number(f64::from(next_id)))
                })
                .expect("install");
        },
        "create(0)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

#[divan::bench]
fn two_number_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("append", 2, append_handler)
                .expect("install");
        },
        "append(1, 2)",
    );
    bencher.bench_local(|| realm.call(&run, Some(&undefined), &[]).expect("run"));
}

#[divan::bench]
fn string_and_number_arguments(bencher: divan::Bencher) {
    let (mut realm, run, undefined) = driver(
        |realm| {
            realm
                .define_global_function("create", 2, |_| Ok(HostValue::Undefined))
                .expect("install");
        },
        "create('view', 2)",
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

/// The other direction: the host calling a member the realm published.
///
/// This is the event path — one call per node an event reaches, with the
/// event's name and JSON detail as arguments. Unlike the cases above, the
/// driver is the host, so each iteration is `CALLS` boundary crossings made
/// from Rust rather than from JavaScript.
fn member_driver(body: &str) -> (Context, Value, quickjs_rust_bridge::Member) {
    let mut realm = Runtime::new()
        .expect("runtime")
        .create_context()
        .expect("realm");
    let host = realm
        .evaluate(
            EvalSource::new(&format!(
                "globalThis.host = {{ sink: 0 }}; host.deliver = {body}; globalThis.host"
            )),
            EvalOptions::default(),
        )
        .expect("install the member");
    let member = realm.member("deliver").expect("intern the member name");
    (realm, host, member)
}

fn drive_member(
    bencher: divan::Bencher,
    body: &str,
    arguments: impl Fn() -> Vec<HostArgument<'static>> + Sync,
) {
    let (mut realm, host, member) = member_driver(body);
    let arguments = arguments();
    bencher.bench_local(|| {
        for _ in 0..CALLS {
            realm
                .call_member(&host, &member, &arguments)
                .expect("deliver");
        }
    });
}

#[divan::bench]
fn member_no_arguments(bencher: divan::Bencher) {
    drive_member(bencher, "function () { host.sink += 1; }", Vec::new);
}

#[divan::bench]
fn member_two_number_arguments(bencher: divan::Bencher) {
    drive_member(bencher, "function (a, b) { host.sink += a + b; }", || {
        vec![HostArgument::Number(17.0), HostArgument::Number(4.0)]
    });
}

/// The `event_listener_callback` shape: two node handles, a capture flag, the
/// event name, its JSON detail, the walk id, and whether this step is last.
#[divan::bench]
fn member_event_arguments(bencher: divan::Bencher) {
    drive_member(
        bencher,
        "function (node, target, capture, name, detail, id, last) { \
           host.sink += node + target + capture + name.length + detail.length + id + last; }",
        || {
            vec![
                HostArgument::Number(4_294_967_298.0),
                HostArgument::Number(4_294_967_299.0),
                HostArgument::Number(0.0),
                HostArgument::String("pointermove"),
                HostArgument::String(r#"{"x":123.5,"y":456.25}"#),
                HostArgument::Number(7.0),
                HostArgument::Boolean(true),
            ]
        },
    );
}
