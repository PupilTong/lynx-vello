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
use dom::event::EventSteps;

fn main() {
    divan::main();
}

/// Rows in a list-shaped tree. Large enough that per-element costs dominate
/// the fixed cost of entering the realm, small enough to stay a screenful of
/// plausible content.
const ROWS: usize = 200;

/// The boot every case starts from: a page, a container, and `ROWS` rows each
/// holding a label, with every element retained on the JavaScript side the way
/// a mounted `ReactLynx` component's fibers retain theirs.
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

/// The first render: `ROWS` rows created, styled, attributed and appended,
/// then committed through style and layout.
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

/// Re-applying a complete inline-style record to every row.
///
/// This is the shape a `ReactLynx` re-render produces for a style prop that
/// changed: one whole record per element, replacing the block rather than
/// mutating it.
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

/// Writing one attribute on every row: the narrowest possible host call, at
/// list scale.
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

/// Reading a string back out of every row.
///
/// This is the host-to-script string direction — the one a returned
/// `HostValue::String` takes on its way into the realm — at list scale.
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

/// A booted list with a listener on the container and one on the page, and
/// the path an event on the deepest label takes.
fn listening_harness() -> (ScriptHarness, EventSteps) {
    let mut harness = ScriptHarness::new();
    harness.boot(&snapshot_source(ROWS));
    harness.evaluate(
        r"
        globalThis.seen = 0;
        __AddEventListener(list, 'tap', () => { seen += 1; }, {});
        __AddEventListener(page, 'tap', () => { seen += 1; }, { capture: true });
        ",
    );
    // The label inside the last row: the deepest node in this shape, so the
    // path is as long as it gets. Named by position rather than by id, so the
    // case does not depend on the order the PAPI issued handles in.
    let page = harness.page();
    let list = harness.child(page, 0).expect("the boot appended the list");
    let bottom_row = harness
        .child(list, harness.child_count(list) - 1)
        .expect("the boot appended rows");
    let label = harness
        .child(bottom_row, 0)
        .expect("every row holds a label");
    let path = harness.event_path(label);
    (harness, path)
}

/// One event walked to two listeners along a full-depth path.
#[divan::bench]
fn dispatch_to_listeners(bencher: divan::Bencher) {
    let name: Arc<str> = Arc::from("tap");
    let detail: Arc<str> = Arc::from(r#"{"x":123.5,"y":456.25}"#);
    bencher
        .with_inputs(listening_harness)
        .bench_local_refs(|(harness, path)| {
            assert!(harness.dispatch(path, &name, &detail));
        });
}

/// The same path for an event name nothing listens to.
///
/// The presenting side answers this from the shared listener-name table, so
/// the realistic cost is one lookup; the dispatch below is what remains if it
/// ever crosses anyway.
#[divan::bench]
fn dispatch_with_no_listener(bencher: divan::Bencher) {
    let name: Arc<str> = Arc::from("scroll");
    let detail: Arc<str> = Arc::from(r#"{"x":123.5,"y":456.25}"#);
    bencher
        .with_inputs(listening_harness)
        .bench_local_refs(|(harness, path)| {
            assert!(!harness.has_listeners(&name));
            assert!(!harness.dispatch(path, &name, &detail));
        });
}

/// Registering and releasing a listener on every row.
///
/// A list that rebuilds its rows does exactly this, and the release path is
/// the one that used to scan every registered event name.
#[divan::bench]
fn register_and_release_row_listeners(bencher: divan::Bencher) {
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
                import {
                  enableEventListener,
                  releaseElement,
                } from 'bobcat-internal:host';
                for (let i = 0; i < rows.length; i += 1) {
                  const id = __GetElementUniqueID(rows[i]);
                  enableEventListener(id, 0, 'tap');
                  enableEventListener(id, 1, 'longpress');
                }
                // `releaseElement`, not `disableEventListener`: this is the
                // call the collector makes for every handle a list update
                // drops, and the one the reverse index exists for. The rows
                // stay attached, so their parent keeps them alive and the
                // next iteration re-registers on the same elements.
                for (let i = 0; i < rows.length; i += 1) {
                  releaseElement(__GetElementUniqueID(rows[i]));
                }
                ",
            );
        });
}
