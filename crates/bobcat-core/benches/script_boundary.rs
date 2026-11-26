//! The composed script boundary, `CodSpeed`-compatible.
//!
//! The bridge's own benchmarks measure one crossing in isolation. These
//! measure what a card actually does: a realm running the Element PAPI, host
//! members reaching a real Lynx document, style and layout committing, and an
//! event path walking back into the realm. Every case is one whole
//! ReactLynx-shaped operation, not a microbenchmark of a member.
//!
//! Each case boots its own realm outside the timed region, so what is timed is
//! the operation and not `QuickJS` startup or the PAPI's own evaluation.

use std::sync::Arc;

use bobcat_core::bench_support::ScriptHarness;

fn main() {
    divan::main();
}

const ROWS: usize = 200;

fn snapshot_source(rows: usize) -> String {
    format!(
        r"
        globalThis.rows = [];
        globalThis.labels = [];
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const list = __CreateView(0);
          __SetInlineStyles(list, {{ display: 'linear', width: '100%' }});
          __AppendElement(page, list);
          globalThis.list = list;
          globalThis.page = page;
          for (let i = 0; i < {rows}; i += 1) {{
            const row = __CreateView(0);
            __SetClasses(row, 'row');
            __SetInlineStyles(row, {{
              width: '100%',
              height: '44px',
              paddingLeft: '12px',
              backgroundColor: 'white',
            }});
            const label = __CreateText(0);
            __SetAttribute(label, 'text', 'row ' + i);
            __AppendElement(row, label);
            __AppendElement(list, row);
            rows.push(row);
            labels.push(label);
          }}
          __FlushElementTree();
        }};
        "
    )
}

#[divan::bench]
fn boot_realm_over_empty_entry(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut harness = ScriptHarness::new();
        harness.boot("");
        harness
    });
}

#[divan::bench]
fn snapshot_first_render(bencher: divan::Bencher) {
    let source = snapshot_source(ROWS);
    bencher
        .with_inputs(ScriptHarness::new)
        .bench_local_values(|mut harness| {
            harness.boot(&source);
            harness
        });
}

#[divan::bench]
fn restyle_every_row(bencher: divan::Bencher) {
    let source = snapshot_source(ROWS);
    bencher
        .with_inputs(|| {
            let mut harness = ScriptHarness::new();
            harness.boot(&source);
            harness
        })
        .bench_local_refs(|harness| {
            harness.evaluate(
                r"
                for (let i = 0; i < rows.length; i += 1) {
                  __SetInlineStyles(rows[i], {
                    width: '100%',
                    height: '48px',
                    paddingLeft: '16px',
                    backgroundColor: i % 2 ? 'gainsboro' : 'white',
                  });
                }
                __FlushElementTree();
                ",
            );
        });
}

#[divan::bench]
fn set_one_attribute_on_every_row(bencher: divan::Bencher) {
    let source = snapshot_source(ROWS);
    bencher
        .with_inputs(|| {
            let mut harness = ScriptHarness::new();
            harness.boot(&source);
            harness
        })
        .bench_local_refs(|harness| {
            harness.evaluate(
                r"
                for (let i = 0; i < labels.length; i += 1) {
                  __SetAttribute(labels[i], 'text', 'updated ' + i);
                }
                __FlushElementTree();
                ",
            );
        });
}

#[divan::bench]
fn read_one_string_from_every_row(bencher: divan::Bencher) {
    let source = snapshot_source(ROWS);
    bencher
        .with_inputs(|| {
            let mut harness = ScriptHarness::new();
            harness.boot(&source);
            harness.evaluate(
                r"
                for (let i = 0; i < rows.length; i += 1) {
                  __SetID(rows[i], 'row-identifier-' + i);
                }
                ",
            );
            harness
        })
        .bench_local_refs(|harness| {
            harness.evaluate(
                r"
                let total = 0;
                for (let i = 0; i < rows.length; i += 1) {
                  total += __GetID(rows[i]).length + __GetTag(rows[i]).length;
                }
                globalThis.total = total;
                ",
            );
        });
}

fn listening_harness() -> (ScriptHarness, u64) {
    let mut harness = ScriptHarness::new();
    harness.boot(&snapshot_source(ROWS));
    harness.evaluate(
        r"
        globalThis.seen = 0;
        __AddEventListener(list, 'tap', () => { seen += 1; }, {});
        __AddEventListener(page, 'tap', () => { seen += 1; }, { capture: true });
        ",
    );
    let page = harness.page();
    let list = harness.child(page, 0).expect("the boot appended the list");
    let last_row = harness.child_count(list) - 1;
    let bottom_row = harness
        .child(list, last_row)
        .expect("the boot appended rows");
    let label = harness
        .child(bottom_row, 0)
        .expect("every row holds a label");
    (harness, label)
}

#[divan::bench]
fn dispatch_to_listeners(bencher: divan::Bencher) {
    let name: Arc<str> = Arc::from("tap");
    bencher
        .with_inputs(listening_harness)
        .bench_local_refs(|(harness, label)| {
            assert!(harness.dispatch(*label, &name));
        });
}

#[divan::bench]
fn dispatch_with_no_listener(bencher: divan::Bencher) {
    let name: Arc<str> = Arc::from("scroll");
    bencher
        .with_inputs(listening_harness)
        .bench_local_refs(|(harness, label)| {
            assert!(!harness.has_listeners(&name));
            assert!(!harness.dispatch(*label, &name));
        });
}

#[divan::bench]
fn register_then_drop_row_listeners(bencher: divan::Bencher) {
    let source = snapshot_source(ROWS);
    bencher
        .with_inputs(|| {
            let mut harness = ScriptHarness::new();
            harness.boot(&source);
            harness
        })
        .bench_local_values(|mut harness| {
            harness.evaluate(
                r"
                import { dropElement } from 'bobcat-internal:host';
                const listener = () => {};
                for (let i = 0; i < rows.length; i += 1) {
                  __AddEventListener(rows[i], 'tap', listener, {});
                  __AddEventListener(rows[i], 'longpress', listener, { capture: true });
                }
                // `dropElement` straight: this is the call the collector
                // makes for every handle a list update drops.
                // The label under each row is an element child, so the drop
                // unlinks it rather than freeing it, and its own handle
                // carries it on.
                for (let i = 0; i < rows.length; i += 1) {
                  __RemoveElement(list, rows[i]);
                  dropElement(__GetElementUniqueID(rows[i]));
                }
                ",
            );
            harness
        });
}

#[divan::bench]
fn flush_one_attribute_update(bencher: divan::Bencher) {
    let source = snapshot_source(ROWS);
    bencher
        .with_inputs(|| {
            let mut harness = ScriptHarness::new();
            harness.boot(&source);
            harness
        })
        .bench_local_refs(|harness| {
            harness.evaluate(
                r"
                globalThis.tick = (globalThis.tick ?? 0) + 1;
                __SetAttribute(rows[0], 'title', 'row ' + tick);
                __FlushElementTree(page, {});
                ",
            );
        });
}

#[divan::bench]
fn flush_one_update_with_timing_flag(bencher: divan::Bencher) {
    let source = snapshot_source(ROWS);
    bencher
        .with_inputs(|| {
            let mut harness = ScriptHarness::new();
            harness.boot(&source);
            harness
        })
        .bench_local_refs(|harness| {
            harness.evaluate(
                r"
                globalThis.tick = (globalThis.tick ?? 0) + 1;
                const options = lynx.performance._generatePipelineOptions();
                options.needTimestamps = true;
                lynx.performance._onPipelineStart(options.pipelineID, options);
                __SetAttribute(rows[0], '__lynx_timing_flag', 'bench');
                __SetAttribute(rows[0], 'title', 'row ' + tick);
                __FlushElementTree(page, { pipelineOptions: options });
                ",
            );
        });
}
