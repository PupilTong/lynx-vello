# Style architecture

The style layer has one standards-oriented core and one Lynx adapter:

```text
lynx-widget  ───────▶  w3c-dom  ───────▶  vendor/stylo
Lynx policy            DOM + CSS core     parser/cascade primitives
```

The previous standalone `lynx-style` crate has been removed; its generic
stylesheet/matching/cascade implementation lives in the DOM core, its
Lynx-only device and unit behavior in `lynx-widget`. The core itself was
subsequently rebuilt from the arena-based `stylo-dom` into `w3c-dom`, a
Document/Node design (this document describes the current shape).

![Style architecture before and after](img/style-architecture-refactor.svg)

## The w3c-dom core: one tree, Document-mediated mutation

- **ONE TREE policy.** `Document<T>` owns one fixed-address `Box<Slab<Node<T>>>`. Slot zero is the
  real `NodeData::Document`; its ordinary child list contains the optional root element, and its
  node data owns the private style context (`SharedRwLock` + base URL). All later slots are element
  or text nodes, addressed by their raw `usize` slab index. Each
  pending pre-mutation snapshot is owned by its node in a pointer-sized optional box, allocated
  only when a previously styled element is first mutated between flushes. There is no separate
  arena/tree object, and no public way to construct or mutate a `Node<T>` outside its document —
  `Document::create_element` and `Document::create_text_node` are the kind-specific factories,
  and every DOM operation is a `Document` method.
- **Invalidation is carried by the operations.** Each matching-relevant setter
  (`set_classes`, `set_attribute`, `set_state`, `set_inline_style`, structural
  `insert_before`/`detach`/`remove_subtree`, …) records its own pre-mutation snapshot or scoped
  restyle hint before touching the node. "Snapshot before mutating" is enforced by construction,
  not asked of embedders. The one embedder obligation left: pair `Document::ext_mut` with
  `Document::note_external_attribute_change` when a payload change affects a synthetic /
  reflected attribute (e.g. Lynx's `l-css-id`, `data-*`).
- **Let it crash.** Query methods return `Option`; mutation methods treat vacant/out-of-range
  `NodeId`s,
  cycle-creating links, a second document element, and invalid insertion references as
  caller bugs — `debug_assert!`ed and panicking rather than silently ignored. Layers holding
  untrusted handles validate first (`WidgetTree` maps violations to `WidgetError`, including its
  Lynx-specific `<page>` root protection).
- **Identity and lifetime are context-owned.** `NodeId` is a raw `usize` slab index. It carries no
  document token and no allocation generation: after a node is removed and its slot reused, the
  same number names the new occupant. Separate JS contexts do not exchange handles, and a native
  `WidgetHandle` carries its context's `Reaper` owner while retaining its node, so no live handle
  survives reclamation and a host-side routing bug is rejected outside the DOM. Engine/document
  pairing similarly uses their shared `Arc<SharedRwLock>` identity rather than an integer token.
- **Debug contract instrumentation.** The `stylo_data` `UnsafeCell` slot carries a debug-only
  guard (reader/writer state, owning thread, unwind poisoning) and the document a debug
  traversal-phase flag; violations of stylo's one-worker-per-element discipline crash debug
  builds instead of being UB. Release builds compile it all away.
- **Slab backpointers, one-word handles, no mirror tree.** Every node carries a pointer directly
  to the fixed-address slab, so it can resolve parents/children and recover slot zero using only
  `&Node`. The same **`&'a Node<T>`** implements Stylo's `TNode`, `TElement`, `TDocument`, and
  `TShadowRoot` associated-type stub; `NodeData` decides whether that node is the document, an
  element, or text. No `Core`, document/node view, or iterator adapter exists. The
  restyle traversal runs **in place on the document**; no second tree is materialized. Text nodes
  remain in DOM/layout child iteration but are skipped by selector matching and cascade. The
  word-sized `TElement` handle is load-bearing: stylo's style-sharing cache sizes its TLS for a
  one-word handle (`FakeCandidate` in `style/sharing/mod.rs`), and a shared reference is exactly
  that (and `Copy` by nature).

## Ownership boundaries

| Layer | Owns | Must not own |
| --- | --- | --- |
| `w3c-dom` | `Document<T>` (fixed-address node slab), slot-zero document `Node<T>` (style context + ordinary child list), element/text nodes (including node-owned pending snapshots), raw-index `NodeId`, direct `&Node` Stylo DOM traits, invalidation-carrying mutation, inline parsing, `Stylist`, rule matching, cascade, media evaluation, computed values | Lynx tags/PAPI, `<page>` root policy, `WidgetState`, Lynx unit metrics, touch-device policy |
| `lynx-widget` | `WidgetState`, `WidgetTree` (PAPI validation plus its own `<page>` root over the generic document), `WidgetHandle` (canonical registry, context ownership, node retention, drop-driven reclamation of detached subtrees), `EngineMetrics`, touch-first `Device` construction, viewport-relative `rpx` integration | A second stylist, cascade implementation, stylesheet lock sharing, direct node construction, raw-id public APIs |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, the maintained Lynx CSS extension patch set **and the Lynx supported-property/value grammar definition** (`style/properties/lynx_properties.txt`, `lynx` feature gates) | Runtime Widget/PAPI policy |

## Style lifecycle

1. `lynx_widget::StyleEngine::new(EngineMetrics)` (or `with_page_config`)
   constructs the touch-first stylo `Device` — its viewport is the `rpx`
   basis — and installs the **UA-origin default sheet** generated from the
   `PageConfig` (`defaultDisplayLinear`, `defaultOverflowVisible`; see
   `crates/lynx-widget/src/ua.rs`). Page config is never an engine branch.
2. The adapter constructs `w3c_dom::StyleEngine`, which owns one `Stylist`,
   one base URL, and one private `SharedRwLock`.
3. `StyleEngine::load_style_info(&StyleInfo)` ingests a decoded bundle by
   **direct construction** (`crates/lynx-widget/src/ingest.rs`): one selector
   parse per rule + per-property value parses into stylo rule objects — no
   CSS-text re-serialization. Lynx policy applied at ingest: `@import`
   flattening (Kahn, web-core parity) and cssId scoping via
   `:where([l-css-id="N"])` guards on the subject compound. The rules mount
   through the fork's `StylesheetContents::from_rules` +
   `w3c_dom::StyleEngine::append_rules`.
4. `StyleEngine::new_widget_tree()` asks the core for a document bound to
   that private style context (`w3c_dom::StyleEngine::new_document`). Neither
   `lynx-widget` nor callers receive the lock. `WidgetTree::create_page`
   records `<page>` as the Lynx-layer root and attaches that ordinary element
   beneath the generic DOM document node.
5. DOM mutations schedule style work as part of the `Document` methods that
   perform them (`crates/w3c-dom/src/invalidation.rs`): attribute / class /
   id / pseudo-state changes record a **pre-mutation snapshot on the affected
   node** for stylo's invalidation sets; structural changes post **restyle
   hints** scoped by the selector flags stylo recorded during matching;
   inline-style updates post the style-attribute replacement hint.
6. `StyleEngine::flush_widget_tree(&mut tree)` drives **stylo's own restyle
   traversal** (`crates/w3c-dom/src/flush.rs`) from `Document::root_element()`, the first element
   child of the slot-zero document node:
   reachable pending node snapshots are moved along the dirty spine into the
   temporary map required by stylo's traversal API, followed by
   snapshot-driven invalidation, the style sharing cache, bloom filter, and
   rayon parallelism over wide DOM levels (stylo's global style pool).
   Computed styles land in each element node's stylo `ElementData`; read them with
   `WidgetTree::computed` (an `Arc<ComputedValues>` clone — direct Arc reads
   per `docs/style-assumptions.md` §B.8).
7. `w3c_dom::StyleEngine::resolve` remains as the standalone per-element
   match+cascade (no traversal state); the Widget adapter exposes it as
   `resolve_widget`.

## Invariants

- Build styled documents through the engine that will resolve them.
  `Document::new` and `WidgetTree::new` are for standalone DOM-only use.
- `SharedRwLock` is an implementation detail of `w3c-dom`; embedders do not
  construct, pass, or read it.
- Standard CSS behavior belongs in `w3c-dom`. Lynx-only extensions and
  environment policy belong in `lynx-widget` (or the maintained stylo fork
  when they are first-class CSS grammar/value extensions — the fork's
  `lynx_properties.txt` + `lynx` feature gates are the source of truth for
  which properties/values the Lynx grammar supports).
- Device mutations go through `w3c_dom::StyleEngine::update_device` or
  `set_viewport`, ensuring media-dependent cascade data is refreshed. After a
  device change the embedder calls
  `lynx_widget::StyleEngine::restyle_after_device_change` on each styled tree
  so `rpx`/`vw`/`vh` lengths re-resolve and media-dependent rules re-match on
  the next flush.
- Snapshot-before-mutate is **internal** to the `Document` setters; the only
  embedder-side pairing is `note_external_attribute_change` before an
  `ext_mut` that changes a synthetic / reflected attribute.
- Node state stylo touches through `&self` during a traversal is atomic; the
  `ElementData` slot is single-owner under stylo's traversal discipline
  (`SAFETY` notes in `w3c-dom`'s `traits`/`flush`). Concurrent parallel
  flushes are serialized process-wide (stylo's global pool keeps
  per-traversal state in worker TLS). This discipline is what upcoming
  parallel style resolving relies on — do not add non-atomic `&self`
  mutability to `Node`.

## Performance posture (see `docs/style-assumptions.md`)

- Ingestion: direct construction, §B.5. Parallel traversal from day 1, §B.6.
- Incremental restyles ride stylo invalidation sets, §B.7 — a class flip on
  one element restyles only affected elements (~3µs per logical operation on
  a 1.1k-widget tree, vs ~1.1ms for the initial full flush). The Divan benches
  batch short operations into millisecond-scale samples and expose the batch
  size through an item counter, so this per-operation figure is derived from
  throughput rather than a flaky microsecond sample.
- Benchmarks: `cargo bench -p lynx-widget` (`benches/style.rs`,
  CodSpeed-tracked) — ingestion, initial flush (sequential + parallel),
  incremental class flip / inline style, no-op flush floor, standalone
  resolve baseline — plus `cargo bench -p w3c-dom` (`benches/css.rs`) at the
  engine level. No native-C++-Lynx comparison harness yet (§E.18 is the bar;
  harness is follow-up work).
