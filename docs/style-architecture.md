# Style architecture

The repository contains one standards-oriented DOM/CSS core, and one Lynx
runtime layer above it:

```text
bobcat-quickjs  ───▶  lynx-element  ───▶  dom  ───▶  vendor/stylo
MTS + host globals    Element PAPI +      DOM +      parser/cascade
                      Lynx policy         CSS core   primitives
```

`dom` owns the generic document, styling, invalidation, and layout seam.
`lynx-element` is the runtime adapter that was drawn dashed here until it
landed: it provides Lynx Element-PAPI policy, view/device configuration, and
UA defaults without moving those concerns into the standards core. Decoded
`.web.bundle` style ingestion is still unbuilt — the seam is
`ElementTree::add_author_stylesheet`.

## The dom core: one tree, Document-mediated mutation

- **One tree, four aligned arenas.** `Document<T>` owns a fixed-address boxed
  `TreeArenas<T>` whose primary `Slab<Node<T>>` selects every `NodeId`: slot
  zero is the real `NodeData::Document`, and later slots are element/text
  nodes. Its payload and Stylo traversal/invalidation slabs share those IDs.
  A separate inline `DocumentLayoutState` owns the fourth, layout/text-state
  slab; nodes point only to the fixed tree arenas, so moving the document does
  not invalidate a layout-state address. Every side insertion asserts that it
  received the primary slab's key, and removal clears all four entries before
  an ID can be reused. Computed styles remain in Stylo-owned node data;
  durable rounded/unrounded layouts live in each state-owned `LayoutSlot`.
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
  by `dom`.
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
| `dom` | `Document<T>` and its aligned arenas; DOM topology and attributes; private style context; invalidation-carrying mutation; inline parsing; matching, cascade, media evaluation, computed values; `StyleDamage`/`FlushSummary`; the concrete `hughie` host and layout-cache invalidation | Lynx tags or Element-PAPI opcodes, JS handle lifetime, payload semantics, `<page>` policy, bundle decoding/`StyleInfo` lowering, Lynx UA defaults, view metrics, touch-device policy |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, and the maintained Lynx CSS extension grammar behind the `lynx` feature | Runtime protocol, document ownership, bundle ingestion, or host policy |
| `lynx-element` (the runtime adapter) | Element-PAPI validation and context-owned handles; Lynx node payload; `<page>` root policy; view metrics and device construction; UA stylesheet generation | A second DOM, matcher, cascade, layout engine, or direct writes to traversal/computed-style internals |
| Still unowned | Lynx event payload; decoded `StyleInfo` lowering and CSS-scope policy; `rpx` view units; the remaining 57 Element PAPI members | — |

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
   `hughie` host. Computed values are lent directly from each node's
   Stylo `ElementData`, without an adapter-side style copy.

## Runtime integration status

`lynx-element` now exposes the Lynx Element PAPI over a `Document`, and
`bobcat-quickjs`'s `mainthread` module runs a `.web.bundle`'s main-thread
script against it. What that covers, and what it does not:

**Landed**

- Lynx page defaults (`display: linear`, border-box, hidden overflow) are
  installed as a UA stylesheet, under the `defaultDisplayLinear` and
  `defaultOverflowVisible` page-config switches;
- view metrics and touch-first device construction (`Viewport::device`);
- Lynx element identity (a dense, never-recycled unique-id handle space) and
  untrusted-handle validation on every PAPI entry point;
- four Element PAPI members — `__CreatePage`, `__CreateView`,
  `__AppendElement`, `__FlushElementTree` — and web-core's boot sequence.

**Still open**

- `.web.bundle` `StyleInfo` decoding exists, but no runtime layer lowers and
  mounts those decoded rules; the seam is
  `ElementTree::add_author_stylesheet`;
- viewport-relative `rpx`/`ppx` units have no owner;
- event registrations, detached-subtree lifetime, CSS-scope (`__SetCSSId`)
  ingestion, and the remaining 57 PAPI members have no adapter;
- `bobcat-engine::view::LynxView` is still not connected to a `Document` —
  `MainThreadRuntime` owns its element tree directly.

These remain responsibilities of the layer above, not to absorb into `dom`,
`bobcat-engine`, or `hughie`.

## Invariants

- Every `Document` owns one complete, private style context.
- Snapshot-before-mutate remains internal to document setters.
- Selector matching reads only real DOM state, never opaque payload fields.
- A successful flush harvests and clears all traversal state it consumed.
- Relayout damage is converted to layout invalidation before a flush summary
  is returned or discarded.
- Standard CSS behavior belongs in `dom`; Lynx-only runtime policy belongs
  above it or in the maintained Stylo fork when it is grammar/value behavior.
- No JS-facing code may expose raw `NodeId` values without a context and
  lifetime layer.

## Validation

- Core tests: `cargo test -p dom`
- Core benchmark: `cargo bench -p dom --bench css`
- Workspace checks: `cargo fmt --check`, `cargo clippy --all-targets`, and
  `cargo test --workspace`
