# Style architecture

The style layer has one standards-oriented core and one Lynx adapter:

```text
lynx-widget  ───────▶  stylo-dom  ───────▶  vendor/stylo
Lynx policy            DOM + CSS core       parser/cascade primitives
```

The previous standalone `lynx-style` crate has been removed. Its generic
stylesheet/matching/cascade implementation moved down into `stylo-dom`; its
Lynx-only device and unit behavior moved up into `lynx-widget`.

![Style architecture before and after](img/style-architecture-refactor.svg)

## Ownership boundaries

| Layer | Owns | Must not own |
| --- | --- | --- |
| `stylo-dom` | `Document<T>` and `Node<T>`, address-stable node back-pointers, stylo DOM traits on `&Node<T>`, invalidation, inline parsing, per-document `Stylist`/`Device`, rule matching, cascade, media evaluation, computed values, and the private `SharedRwLock` | Lynx tags/PAPI, `WidgetState`, Lynx unit metrics, touch-device policy |
| `lynx-widget` | `WidgetState`, `WidgetTree`, opaque `Rc<NodeHandle>` identities and their canonical registry/GC, PAPI validation, `EngineMetrics`, touch-first `Device` construction, viewport-relative `rpx` integration | A second stylist, cascade implementation, stylesheet lock sharing; exposing raw `ElementId` to VM/user code |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, the maintained Lynx CSS extension patch set | Runtime Widget/PAPI policy |

## Style lifecycle

1. `lynx_widget::WidgetTree::with_metrics(EngineMetrics)` (or `with_page_config`)
   constructs the touch-first stylo `Device` — its viewport is the `rpx`
   basis — and installs the **UA-origin default sheet** generated from the
   `PageConfig` (`defaultDisplayLinear`, `defaultOverflowVisible`; see
   `crates/lynx-widget/src/ua.rs`). Page config is never an engine branch.
2. The adapter constructs one independent `stylo_dom::Document<WidgetState>`,
   which owns its node storage together with one `Stylist`, `Device`, base URL,
   and private `SharedRwLock`. Multiple documents are independent instances;
   this is not a singleton model.
3. `WidgetTree::load_style_info(&StyleInfo)` ingests a decoded bundle by
   **direct construction** (`crates/lynx-widget/src/ingest.rs`): one selector
   parse per rule + per-property value parses into stylo rule objects — no
   CSS-text re-serialization. Lynx policy applied at ingest: `@import`
   flattening (Kahn, web-core parity) and cssId scoping via
   `:where([l-css-id="N"])` guards on the subject compound. The rules mount
   through the fork's `StylesheetContents::from_rules` +
   `stylo_dom::Document::append_rules`.
4. DOM mutations schedule style work in `Document<T>` (`crates/stylo-dom/src/dirty.rs`):
   attribute / class / id / pseudo-state changes record **pre-mutation
   snapshots** for stylo's invalidation sets; structural changes post
   **restyle hints** scoped by the selector flags stylo recorded during
   matching; inline-style updates post the style-attribute replacement hint.
5. `WidgetTree::flush_styles()` delegates to `Document::flush` and drives **stylo's own restyle
   traversal** (`crates/stylo-dom/src/flush.rs`): snapshot-driven
   invalidation, the style sharing cache, bloom filter, and rayon
   parallelism over wide DOM levels (stylo's global style pool). Computed
   styles land in each element's stylo `ElementData`; read them with
   `WidgetTree::computed` (an `Arc<ComputedValues>` clone — direct Arc reads
   per `docs/style-assumptions.md` §B.8).
6. `stylo_dom::Document::resolve` remains as the standalone per-element
   match+cascade (no traversal state); the Widget adapter exposes it as
   `resolve_widget`.

## Invariants

- Each view's VM, `WidgetTree`, and `Document<WidgetState>` have one common
  owner thread. They are directly owned rather than made concurrently
  accessible through an outer `Rc`/`Arc`, `RefCell`, or lock. The sole
  owner-thread sharing mechanism is node identity: VM/user code sees only
  `Rc<NodeHandle>`, never `ElementId`, and no handle grants direct access to
  the `Document`.
- `WidgetTree` owns exactly one canonical `Rc<NodeHandle>` per live node.
  Additional strong counts are external owners; delayed work either clones
  that strong handle or explicitly stores `Weak<NodeHandle>` and accepts
  upgrade failure. A per-tree `Rc` allocation identity rejects handles from
  another view before their internal slot ids are considered.
- Mutation and flush are separate synchronous phases on that owner thread. A
  flush is non-reentrant and cannot run JavaScript, deliver events, apply
  resource completions, call layout/render code, or otherwise expose a tree
  mutation until its traversal workers have joined and `complete_flush` has
  returned. Stylo's scoped worker traversal is the sole exception to the
  owner-thread rule: it sees frozen topology and ordinary element fields and
  mutates only the per-element/atomic state permitted by Stylo's traversal
  contract.
- Every `Document<T>` owns exactly one node tree and exactly one matching style
  context. Create as many independent documents as needed; no document state is
  global or shared implicitly.
- Nodes are created through `Document::create_element`; there is no public
  unbound `Node` constructor. Detach keeps the node in its document and makes
  no handle stale. There is no arbitrary public Widget/VM `destroy` or
  `drop_subtree`: `WidgetTree::collect_garbage` may physically reclaim only a
  detached subtree for which every canonical handle has strong count one.
  Reclamation is subtree-atomic, so a strong descendant handle retains the
  entire detached subtree and a parent can never cascade-delete a held child.
- Internal `ElementId` is a `Slab` index only in release builds. Debug/test
  builds add a document-wide allocation epoch solely as an invariant detector:
  stale internal queues or accidental raw-id escape fail on slot reuse rather
  than becoming ABA.
  The collector also asserts registry/document cardinality, canonical-handle
  identity, bidirectional topology, and the final strong count immediately
  before reclamation.
- `Document` does not expose `&mut Node`: moving or swapping a whole node could
  invalidate its document back-pointer. Mutation APIs project only ordinary
  fields (`classes_mut`, `attrs_mut`, `ext_mut`, and so on), while topology is
  changed through the tree primitives.
- `SharedRwLock` is an implementation detail of `stylo-dom`; embedders do not
  construct, pass, or read it.
- Standard CSS behavior belongs in `stylo-dom`. Lynx-only extensions and
  environment policy belong in `lynx-widget` (or the maintained stylo fork
  when they are first-class CSS grammar/value extensions).
- Device mutations go through `stylo_dom::Document::update_device` or
  `set_viewport`, ensuring media-dependent cascade data is refreshed.
  `WidgetTree::set_viewport` and `set_device_pixel_ratio` also schedule their
  own page subtree, so `rpx`/`vw`/`vh` lengths re-resolve and media-dependent
  rules re-match on the next flush.
- **Snapshot before mutating**: every matching-relevant mutation API calls
  its `note_*_change` counterpart *before* applying the change, so the
  snapshot holds the old state.
- Element state stylo touches through `&self` during a traversal is atomic;
  the `ElementData` slot is single-owner under stylo's traversal discipline
  (`SAFETY` notes in `stylo-dom`'s `traits`/`flush`). Concurrent parallel
  flushes are serialized process-wide (stylo's global pool keeps
  per-traversal state in worker TLS). Debug builds mirror the traversal phase
  in an atomic visible to workers and attach a mutable-owner token plus reader
  count to every element. Concurrent parent reads remain legal; duplicate
  `process_preorder` ownership, read/write overlap, access from a
  non-traversal thread, and clearing a still-borrowed `ElementDataWrapper`
  panic at the violation site. These diagnostic fields and operations are
  absent from release builds.
- Every live `Node<T>` has a back-pointer to its boxed, address-stable document
  allocation. A style flush changes that document from `IDLE` to `TRAVERSING`,
  using owner-thread `Cell` state in release builds, and poisons it if
  traversal unwinds. This makes the immutable-tree phase required by Stylo's
  parallel workers an explicit runtime invariant without pretending that host
  mutation can race the flush.

## Performance posture (see `docs/style-assumptions.md`)

- Ingestion: direct construction, §B.5. Parallel traversal from day 1, §B.6.
- Incremental restyles ride stylo invalidation sets, §B.7 — a class flip on
  one element restyles only affected elements (~3µs on a 1.1k-widget tree in
  the divan benches, vs ~1.1ms for the initial full flush).
- Benchmarks: `cargo bench -p lynx-widget` (`benches/style.rs`,
  CodSpeed-tracked) — ingestion, initial flush (sequential + parallel),
  incremental class flip / inline style, no-op flush floor, standalone
  resolve baseline. No native-C++-Lynx comparison harness yet (§E.18 is the
  bar; harness is follow-up work).
