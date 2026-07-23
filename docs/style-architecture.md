# Style architecture

The repository currently contains one standards-oriented DOM/CSS core:

```text
future Lynx runtime adapter  ─ ─ ─▶  w3c-dom  ───────▶  vendor/stylo
Element PAPI + Lynx policy          DOM + CSS core     parser/cascade primitives
```

The dashed layer is intentionally not implemented. `w3c-dom` owns the generic
document, styling, invalidation, and layout seam; a future runtime adapter must
provide Lynx Element-PAPI policy, view/device configuration, UA defaults, and
decoded `.web.bundle` style ingestion without moving those concerns into the
standards core.

## The w3c-dom core: one tree, Document-mediated mutation

- **One tree, four aligned arenas.** `Document<T>` owns one fixed-address
  arena set. Its primary `Slab<Node<T>>` selects every `NodeId`: slot zero is
  the real `NodeData::Document`, and later slots are element/text nodes. Three
  secondary `Slab`s store opaque payload, Stylo traversal/invalidation state,
  and layout measurement/out-of-flow state under the same IDs. Every side
  insertion asserts that it received the primary slab's key; removal clears
  all four entries before an ID can be reused. Computed styles and durable
  rounded/unrounded layouts remain on the primary nodes.
- **Each document owns its style context.** `Document::new` constructs a
  private style engine containing the `Stylist`, device, stylesheet set,
  cascade pipeline, base URL, and `SharedRwLock`. Documents cannot share or
  exchange stylesheets, rule objects, or locks accidentally.
- **Mutation carries invalidation.** Matching-relevant setters such as
  `set_classes`, `set_attribute`, `add_element_state`,
  `remove_element_state`, `set_inline_style`, `insert_before`, `detach`, and
  `remove_subtree` record their own pre-mutation snapshots or scoped restyle
  hints before changing the tree. Stylesheet and device operations schedule
  the document root in the same call. Embedders cannot set, clear, or query
  internal traversal dirty state.
- **Payloads are opaque.** The payload arena retains the `T` supplied for
  each element/text node, and `Node<T>::payload` exposes a shared reference.
  The DOM core neither mutates the payload nor derives selector-visible state
  from it. IDs, classes, inline style, CSS scope markers, and dataset entries
  must be ordinary DOM attributes.
- **The public core is crash-on-misuse.** Query methods return `Option`;
  mutation methods treat stale IDs, cycles, a second document element, and
  invalid insertion references as caller bugs. An untrusted runtime protocol
  must validate its handles before calling the DOM.
- **IDs are document-local raw indices.** `NodeId` has no document token or
  allocation generation, and an index may be reused after removal. A future
  JS-facing adapter therefore owns context routing, canonical handles, and
  garbage-collection/lifetime policy. Those guarantees are not synthesized
  by `w3c-dom`.
- **One-word handles, no mirror tree.** Every node points to the fixed arena
  set. The same plain `&Node<T>` implements Stylo's `TNode`, `TElement`,
  `TDocument`, and shadow-root stub traits according to `NodeData`. Styling
  traverses the real document in place; text nodes remain in DOM/layout child
  iteration but are skipped by selector matching and cascade.
- **Debug-only contract checks.** Styling side data guards Stylo's
  one-worker-per-element discipline and traversal phases in debug builds.
  These checks compile away in release builds.

## Ownership boundaries

| Layer | Owns | Must not own |
| --- | --- | --- |
| `w3c-dom` | `Document<T>` and its aligned arenas; DOM topology and attributes; private style context; invalidation-carrying mutation; inline parsing; matching, cascade, media evaluation, computed values; `StyleDamage`/`FlushSummary`; the concrete `neutron-star` host and layout-cache invalidation | Lynx tags or Element-PAPI opcodes, JS handle lifetime, payload semantics, `<page>` policy, bundle decoding/`StyleInfo` lowering, Lynx UA defaults, view metrics, touch-device policy |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, and the maintained Lynx CSS extension grammar behind the `lynx` feature | Runtime protocol, document ownership, bundle ingestion, or host policy |
| Future runtime adapter | Element-PAPI validation and context-owned handles; Lynx node/event payload; `<page>` root policy; view metrics and device construction; UA stylesheet generation; decoded `StyleInfo` lowering and CSS-scope policy | A second DOM, matcher, cascade, layout engine, or direct writes to traversal/computed-style internals |

## Style lifecycle

1. The embedder constructs a Stylo `Device` and passes it to
   `Document::new` (or `Document::with_url_data`). Device construction is
   deliberately outside the generic DOM because viewport, pointer, color,
   font-metric, and `rpx` policy belong to the runtime environment.
2. The document creates its private stylist, stylesheet set, base URL, and
   lock. Callers may add CSS text through document methods or append rule
   objects constructed for that same document context.
3. DOM mutation methods record snapshots/restyle hints internally.
   Selector-visible data lives in the real node fields and attribute map.
4. `Document::flush_styles` drives Stylo traversal from the document element:
   snapshot invalidation, style sharing, bloom filtering, and parallel
   traversal all run in place.
5. Flush harvest copies each visited element's `StyleDamage`, consumes
   relayout-class damage into containment-bounded layout-cache invalidation,
   and then clears Stylo's damage/restyle state. This clearing prevents old
   damage from triggering later no-op traversals.
6. `Document::resolve_style` remains a read-only standalone match/cascade
   helper. It does not write node styles or participate in traversal
   scheduling.
7. `Document::layout` flushes styles before invoking the concrete
   `neutron-star` host. Computed values are lent directly from each node's
   Stylo `ElementData`, without an adapter-side style copy.

## Runtime integration gap

There is currently no crate that exposes Lynx Element-PAPI or connects
`bobcat-engine::view::LynxView` to a `Document`. Consequently:

- `.web.bundle` `StyleInfo` decoding exists, but no runtime layer lowers and
  mounts those decoded rules into `w3c-dom`;
- Lynx page defaults (`display: linear`, border-box, hidden overflow, and
  page-config variants) are not installed as a UA stylesheet;
- view metrics, touch-first device construction, and viewport-relative `rpx`
  updates have no runtime owner;
- Lynx element identity, event registrations, untrusted-handle validation,
  detached-subtree lifetime, and CSS-scope ingestion have no public adapter.

These are explicit future-integration tasks, not responsibilities to absorb
into `w3c-dom`, `bobcat-engine`, or `neutron-star`.

## Invariants

- Every `Document` owns one complete, private style context.
- Snapshot-before-mutate remains internal to document setters.
- Selector matching reads only real DOM state, never opaque payload fields.
- A successful flush harvests and clears all traversal state it consumed.
- Relayout damage is converted to layout invalidation before a flush summary
  is returned or discarded.
- Standard CSS behavior belongs in `w3c-dom`; Lynx-only runtime policy belongs
  above it or in the maintained Stylo fork when it is grammar/value behavior.
- No JS-facing code may expose raw `NodeId` values without a context and
  lifetime layer.

## Validation

- Core tests: `cargo test -p w3c-dom`
- Core benchmark: `cargo bench -p w3c-dom --bench css`
- Workspace checks: `cargo fmt --check`, `cargo clippy --all-targets`, and
  `cargo test --workspace`
