//! JavaScript execution benchmarks for the pinned `QuickJS` interpreter.
//!
//! Realm creation and source compilation happen before each measurement. The
//! measured operation calls an already-compiled JavaScript function, keeping
//! the cases focused on interpreter execution, allocation, and built-ins. The
//! workloads model common application-script shapes rather than isolated C API
//! calls; the latter are covered by `host_calls`.

use divan::black_box;
use divan::counter::ItemsCount;
use quickjs_rust_bridge::{EvalOptions, EvalSource, Realm, Value};

fn main() {
    divan::main();
}

struct Driver {
    realm: Realm,
    run: Value,
    undefined: Value,
}

impl Driver {
    fn compile(body: &str) -> Self {
        let mut realm = Realm::new().expect("realm");
        let source = format!("globalThis.run = function run() {{\n{body}\n}};\nglobalThis.run;");
        let run = realm
            .evaluate(
                EvalSource {
                    text: &source,
                    name: Some("js-execution-bench.js"),
                    line_offset: 0,
                },
                EvalOptions::default(),
            )
            .expect("compile benchmark driver");
        let undefined = realm.undefined().expect("undefined");
        Self {
            realm,
            run,
            undefined,
        }
    }

    fn call(&mut self) -> Value {
        self.realm
            .call(&self.run, Some(&self.undefined), &[])
            .expect("execute benchmark driver")
    }
}

fn substitute(template: &str, constants: &[(&str, usize)]) -> String {
    constants
        .iter()
        .fold(template.to_owned(), |source, (name, value)| {
            source.replace(name, &value.to_string())
        })
}

fn bench_compiled(bencher: divan::Bencher, operations: usize, body: &str) {
    let mut driver = Driver::compile(body);
    bencher
        .counter(ItemsCount::new(operations))
        .bench_local(|| black_box(driver.call()));
}

const ARITHMETIC_ITERATIONS: usize = 100_000;

#[divan::bench]
fn arithmetic_and_branches(bencher: divan::Bencher) {
    let body = substitute(
        r"
        let checksum = 0;
        for (let i = 0; i < __ITERATIONS__; i += 1) {
            checksum = (checksum + ((i * 17) ^ (i >>> 3))) | 0;
            if ((i & 7) === 0) {
                checksum ^= i;
            }
        }
        return checksum;
        ",
        &[("__ITERATIONS__", ARITHMETIC_ITERATIONS)],
    );
    bench_compiled(bencher, ARITHMETIC_ITERATIONS, &body);
}

const FUNCTION_CALLS: usize = 30_000;

#[divan::bench]
fn nested_function_calls(bencher: divan::Bencher) {
    let body = substitute(
        r"
        function mix(value, index) {
            return ((value * 33) ^ index) | 0;
        }
        function step(value, index) {
            return mix(value, index) + (index & 3);
        }
        let result = 1;
        for (let i = 0; i < __CALLS__; i += 1) {
            result = step(result, i);
        }
        return result;
        ",
        &[("__CALLS__", FUNCTION_CALLS)],
    );
    bench_compiled(bencher, FUNCTION_CALLS * 2, &body);
}

const PROPERTY_UPDATES: usize = 50_000;
const RECORDS: usize = 64;

#[divan::bench]
fn object_property_updates(bencher: divan::Bencher) {
    let body = substitute(
        r"
        const records = new Array(__RECORDS__);
        for (let i = 0; i < records.length; i += 1) {
            records[i] = { x: i, y: i * 2, active: true };
        }
        let checksum = 0;
        for (let i = 0; i < __UPDATES__; i += 1) {
            const record = records[i & (__RECORDS__ - 1)];
            record.x = (record.x + record.y + i) | 0;
            record.active = !record.active;
            checksum ^= record.x;
        }
        return checksum;
        ",
        &[("__RECORDS__", RECORDS), ("__UPDATES__", PROPERTY_UPDATES)],
    );
    bench_compiled(bencher, PROPERTY_UPDATES, &body);
}

const ARRAY_LENGTH: usize = 1_024;
const ARRAY_PASSES: usize = 64;

#[divan::bench]
fn dense_array_iteration(bencher: divan::Bencher) {
    let body = substitute(
        r"
        const values = new Array(__LENGTH__);
        for (let i = 0; i < values.length; i += 1) {
            values[i] = i & 255;
        }
        let total = 0;
        for (let pass = 0; pass < __PASSES__; pass += 1) {
            for (let i = 0; i < values.length; i += 1) {
                total = (total + values[i]) | 0;
                values[i] = (values[i] + pass) & 255;
            }
        }
        return total;
        ",
        &[("__LENGTH__", ARRAY_LENGTH), ("__PASSES__", ARRAY_PASSES)],
    );
    bench_compiled(bencher, ARRAY_LENGTH * ARRAY_PASSES, &body);
}

const ALLOCATIONS: usize = 20_000;

#[divan::bench]
fn short_lived_object_allocation(bencher: divan::Bencher) {
    let body = substitute(
        r"
        let checksum = 0;
        for (let i = 0; i < __ALLOCATIONS__; i += 1) {
            const point = { x: i, y: i ^ 0x55aa };
            const tuple = [point.x, point.y, point.x + point.y];
            checksum = (checksum + tuple[2]) | 0;
        }
        return checksum;
        ",
        &[("__ALLOCATIONS__", ALLOCATIONS)],
    );
    bench_compiled(bencher, ALLOCATIONS * 2, &body);
}

const STRING_PASSES: usize = 32;

#[divan::bench]
fn string_hashing(bencher: divan::Bencher) {
    let body = substitute(
        r#"
        const text = "Lynx renders application interfaces through QuickJS. ".repeat(32);
        let hash = 0x811c9dc5;
        for (let pass = 0; pass < __PASSES__; pass += 1) {
            for (let i = 0; i < text.length; i += 1) {
                hash = Math.imul(hash ^ text.charCodeAt(i), 0x01000193);
            }
        }
        return hash;
        "#,
        &[("__PASSES__", STRING_PASSES)],
    );
    bench_compiled(bencher, STRING_PASSES, &body);
}

const JSON_ROUNDS: usize = 300;

#[divan::bench]
fn json_parse_mutate_and_stringify(bencher: divan::Bencher) {
    let body = substitute(
        r#"
        const source = '{"type":"view","props":{"id":"card","class":"primary"},"children":[{"type":"text","value":"hello"},{"type":"image","src":"asset.png"}]}';
        let total = 0;
        for (let i = 0; i < __ROUNDS__; i += 1) {
            const value = JSON.parse(source);
            value.props.index = i;
            value.children[0].value = "row-" + i;
            total += JSON.stringify(value).length;
        }
        return total;
        "#,
        &[("__ROUNDS__", JSON_ROUNDS)],
    );
    bench_compiled(bencher, JSON_ROUNDS, &body);
}

const REGEXP_ROUNDS: usize = 500;

#[divan::bench]
fn regexp_scanning(bencher: divan::Bencher) {
    let body = substitute(
        r#"
        const input = "view-12 text-3 image-44 wrapper-8 ".repeat(16);
        const pattern = /([a-z]+)-(\d+)/g;
        let matches = 0;
        for (let round = 0; round < __ROUNDS__; round += 1) {
            pattern.lastIndex = 0;
            while (pattern.exec(input) !== null) {
                matches += 1;
            }
        }
        return matches;
        "#,
        &[("__ROUNDS__", REGEXP_ROUNDS)],
    );
    bench_compiled(bencher, REGEXP_ROUNDS, &body);
}

const PROMISE_JOBS: usize = 1_000;

#[divan::bench]
fn promise_job_checkpoint(bencher: divan::Bencher) {
    let body = substitute(
        r"
        let promise = Promise.resolve(0);
        for (let i = 0; i < __JOBS__; i += 1) {
            promise = promise.then(value => value + 1);
        }
        promise.then(value => { globalThis.__benchPromiseSink = value; });
        return __JOBS__;
        ",
        &[("__JOBS__", PROMISE_JOBS)],
    );
    let mut driver = Driver::compile(&body);
    bencher
        .counter(ItemsCount::new(PROMISE_JOBS + 1))
        .bench_local(|| {
            drop(driver.call());
            black_box(
                driver
                    .realm
                    .drain_pending_jobs()
                    .expect("drain benchmark promise jobs"),
            )
        });
}
