# Style architecture

CSS computation has exactly one workspace owner: `stylo-dom`.
`lynx-widget` is a PAPI/host-input caller of that core, not a style layer or a
second adapter that shares ownership of CSS semantics:

```text
lynx-widget  ── DOM operations / input data ──▶  stylo-dom  ──▶  vendor/stylo
PAPI facade                                      all CSS          grammar and
                                                 computation      engine primitives
```

The previous standalone `lynx-style` crate has been removed. Its generic
stylesheet/matching/cascade implementation moved into `stylo-dom`. A host may
supply viewport metrics or decoded bundle data through `lynx-widget`, but the
fact that it forwards those inputs does not give it ownership of CSS parsing,
matching, cascade, defaults, computed values, or traversal.

## Ownership boundaries

| Layer | Owns | Must not own |
| --- | --- | --- |
| `stylo-dom` | `Document<T>` and `Node<T>`, address-stable node back-pointers, stylo DOM traits on `&Node<T>`, invalidation, inline parsing, per-document `Stylist`/`Device`, stylesheet origins/default sheets, rule matching, cascade, media/unit evaluation, computed values, traversal, and the private `SharedRwLock` | Lynx tags/PAPI, `WidgetState`, VM handles or widget/layout policy |
| `lynx-widget` | `WidgetState`, `WidgetTree`, opaque `Rc<NodeHandle>` identities and their canonical registry/GC, PAPI validation, and transport of host metrics/configuration or decoded bundle inputs into `Document` APIs | CSS grammar/value decisions, UA/default-style semantics, selector matching, cascade, invalidation/traversal, computed-style calculation, style-to-layout adaptation, a second stylist, or raw `ElementId` exposure |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, the maintained Lynx CSS extension patch set | Runtime Widget/PAPI policy |

## Style lifecycle

1. A host-facing `WidgetTree` can collect viewport/configuration and decoded
   bundle inputs, then pass them to one independent
   `stylo_dom::Document<WidgetState>`. This is API orchestration only: it must
   not interpret property values or calculate style.
2. The `Document` owns its node storage together with one `Stylist`, `Device`,
   stylesheet/default origins, base URL, and private `SharedRwLock`. It is the
   sole owner of parsing semantics, matching, cascade, media/unit evaluation,
   computed values, and style traversal. Multiple documents are independent
   instances; this is not a singleton model.
3. `WidgetTree::load_style_info(&StyleInfo)` is a forwarding entry point for a
   decoded transport representation. The CSS-facing ingestion pipeline is
   owned by `Document`: it uses direct rule construction (one selector parse
   per rule plus per-property value parsing, without CSS-text
   re-serialization), performs `@import` flattening and cssId scoping via
   `:where([l-css-id="N"])` guards, and mounts the resulting rules into its
   own stylist. The widget facade must not decide property names, value
   grammar, selector semantics, cascade origins, or defaults.
4. DOM mutations schedule style work in `Document<T>` (`crates/stylo-dom/src/dirty.rs`):
   attribute / class / id / pseudo-state changes record **pre-mutation
   snapshots** for stylo's invalidation sets; structural changes post
   **restyle hints** scoped by the selector flags stylo recorded during
   matching; inline-style updates post the style-attribute replacement hint.
5. `WidgetTree::flush_styles()` is only a forwarding API. `Document::flush`
   owns and drives **stylo's own restyle traversal**
   (`crates/stylo-dom/src/flush.rs`): snapshot-driven
   invalidation, the style sharing cache, bloom filter, and rayon
   parallelism over wide DOM levels (stylo's global style pool). Computed
   styles land in each element's stylo `ElementData`; read them with
   `WidgetTree::computed` (an `Arc<ComputedValues>` clone — direct Arc reads
   per `docs/style-assumptions.md` §B.8).
6. `stylo_dom::Document::resolve` owns the standalone per-element
   match+cascade (no traversal state); `resolve_widget` is only a facade for
   that document operation.

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
- All CSS behavior belongs to `stylo-dom` plus the maintained Stylo fork:
  standard behavior, Lynx grammar extensions, unit interpretation,
  UA/default-style semantics, matching, cascade, and computed values.
  `lynx-widget` can supply host data and request operations; that is not CSS
  computation and must never be used as a reason to place style behavior in
  the widget crate.
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
- End-to-end facade benchmarks currently run through `cargo bench -p
  lynx-widget` (`benches/style.rs`, CodSpeed-tracked), but the measured CSS
  work is performed by its `stylo-dom::Document`: ingestion, initial flush (sequential + parallel),
  incremental class flip / inline style, no-op flush floor, standalone
  resolve baseline. No native-C++-Lynx comparison harness yet (§E.18 is the
  bar; harness is follow-up work).
