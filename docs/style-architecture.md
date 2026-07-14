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
| `stylo-dom` | `Node<T> = Element | Text`, `Arena<T>`, stylo DOM traits, invalidation, inline parsing, `Stylist`, rule matching, cascade, media evaluation, computed values, the private `SharedRwLock`, and the `DomLayoutSource`/`DomLayoutSession` pair plus lazy computed-style views and queryable layout output | Lynx tags/PAPI, `WidgetState`, Lynx unit metrics, touch-device policy |
| `lynx-widget` | `WidgetState`, `WidgetTree`, PAPI validation, `EngineMetrics`, touch-first `Device` construction, and viewport-relative `rpx` integration | A second stylist, cascade implementation, stylesheet lock sharing; its PAPI-to-layout adapter is deferred |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, the maintained Lynx CSS extension patch set | Runtime Widget/PAPI policy |

## Style lifecycle

1. `lynx_widget::StyleEngine::new(EngineMetrics)` (or `with_page_config`)
   constructs the touch-first stylo `Device` — its viewport is the `rpx`
   basis — and installs the **UA-origin default sheet** generated from the
   `PageConfig` (`defaultDisplayLinear`, `defaultOverflowVisible`; see
   `crates/lynx-widget/src/ua.rs`). Page config is never an engine branch.
2. The adapter constructs `stylo_dom::StyleEngine`, which owns one `Stylist`,
   one base URL, and one private `SharedRwLock`.
3. `StyleEngine::load_style_info(&StyleInfo)` ingests a decoded bundle by
   **direct construction** (`crates/lynx-widget/src/ingest.rs`): one selector
   parse per rule + per-property value parses into stylo rule objects — no
   CSS-text re-serialization. Lynx policy applied at ingest: `@import`
   flattening (Kahn, web-core parity) and cssId scoping via
   `:where([l-css-id="N"])` guards on the subject compound. The rules mount
   through the fork's `StylesheetContents::from_rules` +
   `stylo_dom::StyleEngine::append_rules`.
4. `StyleEngine::new_widget_tree()` asks the core for an arena bound to that
   private style context. Neither `lynx-widget` nor callers receive the lock.
5. DOM mutations schedule style work in `Arena<T>` (`crates/stylo-dom/src/dirty.rs`):
   attribute / class / id / pseudo-state changes record **pre-mutation
   snapshots** for stylo's invalidation sets; structural changes post
   **restyle hints** scoped by the selector flags stylo recorded during
   matching; inline-style updates post the style-attribute replacement hint.
6. `StyleEngine::flush_widget_tree(&mut tree)` drives **stylo's own restyle
   traversal** (`crates/stylo-dom/src/flush.rs`): snapshot-driven
   invalidation, the style sharing cache, bloom filter, and rayon
   parallelism over wide DOM levels (stylo's global style pool). Computed
   styles land in each element's stylo `ElementData`; read them with
   `WidgetTree::computed` (an `Arc<ComputedValues>` clone — direct Arc reads
   per `docs/style-assumptions.md` §B.8).
7. `stylo_dom::StyleEngine::resolve` remains as the standalone per-element
   match+cascade (no traversal state); the Widget adapter exposes it as
   `resolve_widget`.
8. After a flush, `stylo_dom::layout::DomLayoutSource` borrows the `Arena`
   directly and builds only dense formatting metadata plus strong references
   to the relevant computed-style Arcs. Its accessors translate lazily into
   neutron-star values. Real Text nodes carry no computed style of their own;
   contiguous text becomes an anonymous item whose paragraph/run values come
   from the surrounding styled elements. The source records the Arena's
   process-unique identity and conservative `layout_revision`.
9. `stylo_dom::layout::DomLayoutSession` consumes that immutable epoch and
   retains the disjoint mutable box caches, rounded layouts, Parley context and
   text artifacts. After `commit`, `final_layout` and
   `committed_text_layout` expose output by real Element/Text `NodeId`; a Text
   contributor resolves to its shared anonymous item's box and paragraph.
   Successful font registration invalidates both retained measurement state
   and result queries until the next commit.

## Invariants

- Build styled arenas through the engine that will resolve them. `Arena::new`
  and `WidgetTree::new` are for standalone DOM-only use.
- `SharedRwLock` is an implementation detail of `stylo-dom`; embedders do not
  construct, pass, or read it.
- Standard CSS behavior belongs in `stylo-dom`. Lynx-only extensions and
  environment policy belong in `lynx-widget` (or the maintained stylo fork
  when they are first-class CSS grammar/value extensions).
- DOM Text is not represented by a fake element: it has no external payload,
  tag, attributes, or `ElementData`, and `NodeRef`/Stylo `TNode` reports the
  standard Element-vs-Text semantics. `lynx-widget` PAPI projection into this
  DOM/layout model is intentionally deferred.
- Device mutations go through `stylo_dom::StyleEngine::update_device` or
  `set_viewport`, ensuring media-dependent cascade data is refreshed. After a
  device change the embedder calls
  `lynx_widget::StyleEngine::restyle_after_device_change` on each styled tree
  so `rpx`/`vw`/`vh` lengths re-resolve and media-dependent rules re-match on
  the next flush.
- **Snapshot before mutating**: every matching-relevant mutation API calls
  its `note_*_change` counterpart *before* applying the change, so the
  snapshot holds the old state.
- Element state stylo touches through `&self` during a traversal is atomic;
  the `ElementData` slot is single-owner under stylo's traversal discipline
  (`SAFETY` notes in `stylo-dom`'s `traits`/`flush`). Concurrent parallel
  flushes are serialized process-wide (stylo's global pool keeps
  per-traversal state in worker TLS).

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
