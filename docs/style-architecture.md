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

- **ONE TREE policy.** `Document<T>` is the single owner of everything tree-shaped: node storage
  (a generational slot arena), the optional document root, the pending pre-mutation snapshot set,
  and the private style context (`SharedRwLock` + base URL). There is no separate arena/tree
  object, and no public way to construct or mutate a `Node<T>` outside its document —
  `Document::create_node` is the only factory, and every DOM operation is a `Document` method.
- **Invalidation is carried by the operations.** Each matching-relevant setter
  (`set_classes`, `set_attribute`, `set_state`, `set_inline_style`, structural
  `insert_before`/`detach`/`remove_subtree`, …) records its own pre-mutation snapshot or scoped
  restyle hint before touching the node. "Snapshot before mutating" is enforced by construction,
  not asked of embedders. The one embedder obligation left: pair `Document::ext_mut` with
  `Document::note_external_attribute_change` when a payload change affects a synthetic /
  reflected attribute (e.g. Lynx's `l-css-id`, `data-*`).
- **Let it crash.** Query methods return `Option`; mutation methods treat stale `NodeId`s,
  cycle-creating links, and foreign insertion references as caller bugs — `debug_assert!`ed and
  panicking rather than silently ignored. Layers holding untrusted handles validate first
  (`WidgetTree` maps violations to `WidgetError`).
- **Backpointers, one-word handles, no mirror tree.** The document core is heap-pinned and every
  node carries a pointer back to it, so tree navigation needs nothing but `&Node`. stylo's
  element traits (`TNode`/`TElement`/`TDocument`/`selectors::Element`) are implemented on the
  one-word `NodeRef(&Node)` handle, and the restyle traversal runs **in place on the document**
  — no second tree is materialized to enter the styling engine. The word-sized handle is
  load-bearing: stylo's style-sharing cache sizes its TLS for a one-word `TElement` handle
  (`FakeCandidate` in `style/sharing/mod.rs`).

## Ownership boundaries

| Layer | Owns | Must not own |
| --- | --- | --- |
| `w3c-dom` | `Document<T>` (the one tree: storage, root, snapshots, lock), `Node<T>`, `NodeId`/`NodeRef`, stylo DOM traits, invalidation-carrying mutation, inline parsing, `Stylist`, rule matching, cascade, media evaluation, computed values | Lynx tags/PAPI, `WidgetState`, Lynx unit metrics, touch-device policy |
| `lynx-widget` | `WidgetState`, `WidgetTree` (PAPI validation over the document), `EngineMetrics`, touch-first `Device` construction, viewport-relative `rpx` integration | A second stylist, cascade implementation, stylesheet lock sharing, direct node construction |
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
   `lynx-widget` nor callers receive the lock.
5. DOM mutations schedule style work as part of the `Document` methods that
   perform them (`crates/w3c-dom/src/invalidation.rs`): attribute / class /
   id / pseudo-state changes record **pre-mutation snapshots** for stylo's
   invalidation sets; structural changes post **restyle hints** scoped by the
   selector flags stylo recorded during matching; inline-style updates post
   the style-attribute replacement hint.
6. `StyleEngine::flush_widget_tree(&mut tree)` drives **stylo's own restyle
   traversal** (`crates/w3c-dom/src/flush.rs`) from the document root:
   snapshot-driven invalidation, the style sharing cache, bloom filter, and
   rayon parallelism over wide DOM levels (stylo's global style pool).
   Computed styles land in each node's stylo `ElementData`; read them with
   `WidgetTree::computed` (an `Arc<ComputedValues>` clone — direct Arc reads
   per `docs/style-assumptions.md` §B.8).
7. `w3c_dom::StyleEngine::resolve` remains as the standalone per-node
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
  one element restyles only affected elements (~3µs on a 1.1k-widget tree in
  the divan benches, vs ~1.1ms for the initial full flush).
- Benchmarks: `cargo bench -p lynx-widget` (`benches/style.rs`,
  CodSpeed-tracked) — ingestion, initial flush (sequential + parallel),
  incremental class flip / inline style, no-op flush floor, standalone
  resolve baseline — plus `cargo bench -p w3c-dom` (`benches/css.rs`) at the
  engine level. No native-C++-Lynx comparison harness yet (§E.18 is the bar;
  harness is follow-up work).
