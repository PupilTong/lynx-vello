# Style architecture

The repository contains one standards-oriented DOM/CSS core, a rendered
runtime core, and one Lynx policy layer above them:

```text
bobcat-cli ───▶ bobcat-core ───▶ dom ─┬─▶ vendor/stylo
                     ▲                ├─▶ hughie
   packages/bobcat-element            └─▶ vello/wgpu
   (embedded Element PAPI runtime,
    drives bobcat-core's tree module
    through the `bobcat` realm global)
```

`dom` owns the generic document, styling, invalidation, layout seam, visual
order, and private paint pipeline.
The Lynx runtime element layer is split across the boundary it crosses:
`bobcat_core::tree` is the native half — Lynx page policy over
`dom::Document<()>`: view/device configuration and UA defaults — and
`packages/bobcat-element` is the script half, owning Element-PAPI member
policy, tag vocabulary, and handle lifecycle, without moving any of those
concerns into the standards core.
`bobcat-core` composes that tree with runtime protocols and optionally
supplies QuickJS, but re-exports no DOM/GPU or renderer conveniences.
`bobcat-cli` is an independent product that embeds `bobcat-core` only through
the opaque `LynxView` facade and its resource, draw-target, VM, and OS host
contracts. `dom`'s `render` module remains the internal DOM-free resource/GPU
floor; `bobcat-core` does not re-export it.

The audited normal-build surface and every test-feature exception are listed
in [dom-public-api.md](dom-public-api.md).
See
[`runtime-architecture.md`](runtime-architecture.md) for the full dependency,
feature, and frame-flow walkthrough. Decoded `.web.bundle` style ingestion runs
through the rule-construction seam (`Document::build_style_rule` and friends,
then `Document::append_rules`); `Document::add_stylesheet` remains the seam for
CSS supplied as text.

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
- **Each document owns its style context.** `Document::new` constructs a private
  style engine containing the `Stylist`, device, stylesheet set, cascade
  pipeline, base URL, and `SharedRwLock`. Documents cannot share or exchange
  stylesheets, rule objects, or locks accidentally.
- **Mutation carries invalidation.** Matching-relevant setters such as
  `set_classes`, `set_attribute`, `add_element_state`,
  `remove_element_state`, `set_inline_style`, `set_inline_style_property`,
  `insert_before`,
  `remove_element`, `drop_element`, and `drop_subtree` record their own
  pre-mutation snapshots or scoped restyle hints before changing the tree.
  Stylesheet and device operations schedule the document root in the same call.
  Embedders cannot set, clear, or query internal traversal dirty state.
- **Payloads are opaque.** The payload arena retains the `T` supplied for
  each element/text node, and `Node<T>::payload` exposes a shared reference.
  The DOM core neither mutates the payload nor derives selector-visible state
  from it. IDs, classes, inline style, CSS scope markers, and dataset entries
  must be ordinary DOM attributes.
- **The public core is crash-on-misuse.** Query methods return `Option`;
  mutation methods treat stale IDs, cycles, a second document element, and
  invalid insertion references as caller bugs. The private runtime validates
  script-provided primitive/live `NodeId`s and tree-mutation preconditions
  before entering these methods, reporting misuse as a JavaScript error.
- **IDs are document-local raw indices.** `NodeId` has no document token or
  allocation generation, and an index may be reused after removal. The
  Element PAPI runtime's handle objects unmap on drop, so a live handle
  never resolves to a reused slot; anything else that replays a stale id is
  a caller bug.
- **One-word handles, no mirror tree.** Every node points to the fixed arena
  set. The same plain `&Node<T>` implements Stylo's `TNode`, `TElement`,
  `TDocument`, and `TShadowRoot` according to `NodeData`. Styling
  traverses the real document in place; text nodes remain in DOM/layout child
  iteration but are skipped by selector matching and cascade.
- **Three trees, one arena.** A shadow root is a fourth `NodeData` kind,
  attached to its host rather than listed among its children. Selector
  matching climbs the **node** tree, so a combinator simply runs out of
  parents at a shadow root and Stylo retries against the featureless host
  (`:host`). Traversal, inheritance, layout, paint, and hit testing read the
  **flat** tree — hosts replaced by their shadow trees, `<slot>`s by their
  assigned nodes — through `Node::flat_children`/`flat_parent_id`. Each
  shadow root owns the scoped `CascadeData` its tree matches against instead
  of the document's author rules.
- **Custom elements are behavior, not vocabulary.** `Document::define` binds
  one `dyn CustomElement<T>` handler to one injected local name; the DOM core
  owns the state machine (`uncustomized`/`constructing`/`custom`), the
  observed-attribute filter, and the reaction queue. Scope is user-agent
  components: `define` requires every definition to precede its elements, which
  removes the standard's upgrade half, so `ElementState::DEFINED` is seeded at
  creation and never moves.
  Reactions are queued and drained at each public mutation's boundary, so a
  lifecycle callback never observes a half-applied DOM algorithm.
- **Debug-only contract checks.** Styling side data guards Stylo's
  one-worker-per-element discipline and traversal phases in debug builds.
  These checks compile away in release builds.

## Ownership boundaries

| Layer | Owns | Must not own |
| --- | --- | --- |
| `dom` | `Document<T>` and its aligned arenas; DOM topology and attributes; private style context and damage harvest; invalidation-carrying mutation; inline parsing; matching, cascade, media evaluation, computed values; the concrete `hughie` host; private visual order, `Painter`, `ImageStore`, and retained Vello scene | Pluggable renderer policy, Lynx tags or Element-PAPI opcodes, JS handle lifetime, payload semantics, `<page>` policy, bundle decoding/`StyleInfo` lowering, Lynx UA defaults, view metrics, GPU surface/window policy |
| `bobcat-core` | Opaque `LynxView`; injected resource, VM-factory, image-decoder, draw-target, and OS-input contracts; private Lynx page policy (`page` root, device construction, UA stylesheet); private engine/tree/runtime; optional opaque QuickJS factory; the `bobcat` realm object and embedded Element PAPI runtime | Re-exporting `dom`, exposing engine/tree/document/realm handles, bundle decoding or config parsing, an element-host trait, matcher/cascade/layout/paint algorithms, public `PaintOrder`, or the PAPI member surface itself (that is `packages/bobcat-element`'s) |
| `dom::render` (the DOM-free floor) | Opaque `ImageStore`; Vello version/re-export boundary; headed/headless GPU submission and readback helpers | `Document`, `NodeId`, computed styles, layout, paint order, Lynx runtime vocabulary, or DOM mutation policy |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, and the maintained Lynx CSS extension grammar behind the `lynx` feature | Runtime protocol, document ownership, bundle ingestion, or host policy |
| `packages/bobcat-element` (the script half) | The twenty-six `__*` Element-PAPI members and their arities; Lynx tag vocabulary; handle identity (one plain object per element, carrying its DOM `NodeId` under a realm-local symbol — web-core's `uniqueIdSymbol` shape); Snapshot property/query policy; realm-local event registration; the `FinalizationRegistry` drop backstop (cleanup calls `bobcat.dropElement` at the host's job checkpoints) | Native-ID validation, style/layout/paint behavior, direct DOM access, event dispatch, or any state the native side must gate presentation on |
| Still unowned | Lynx event dispatch/payload; decoded `StyleInfo` lowering and CSS-scope policy; `rpx` view units; the remaining Element PAPI members | — |

## Style lifecycle

1. Private `bobcat_core::tree::new_document` constructs a Stylo `Device` and creates
   `dom::Document<()>` through `Document::new`. Device construction is deliberately outside the
   generic DOM because viewport, pointer, color, font-metric, and `rpx` policy
   belong to the runtime environment.
2. The document creates its private stylist, stylesheet set, `about:blank`
   base URL, and lock. Callers add complete CSS text through
   `Document::add_stylesheet`, or — for CSS a host already parsed — build rules
   with `Document::build_style_rule`/`build_keyframes_rule`/`build_font_face_rule`
   and mount them with `Document::append_rules`. Locks and base URLs still never
   cross the API: a `dom::CssRule` is opaque and carries the lock that minted
   it, and `append_rules` rejects a rule built by a different document.
3. DOM mutation methods record snapshots/restyle hints internally.
   Selector-visible data lives in the real node fields and attribute map.
4. `Document::layout` first drives Stylo traversal from the document element:
   snapshot invalidation, style sharing, bloom filtering, and parallel
   traversal all run in place. Standalone flush/damage inspection is private
   to crate unit tests; external tests and benchmarks use the production
   commit path.
5. The internal harvest reads each visited element's `StyleDamage`, consumes
   relayout-class damage into containment-bounded layout-cache invalidation,
   and then clears Stylo's damage/restyle state. This clearing prevents old
   damage from triggering later no-op traversals.
6. `Document::layout` then invokes the concrete
   `hughie` host. Computed values are lent directly from each node's
   Stylo `ElementData`, without an adapter-side style copy.
7. Bobcat's private `Engine` asks the document-owned Painter (through
   the shared document) whether its retained scene is
   current. A dirty document runs `Document::render`, builds the private
   paint order, and retains the resulting Vello scene. Embedders never drive
   this lifecycle — they relay OS facts to the engine, which schedules it.

## Runtime integration status

Private `bobcat_core::tree` composes the native operations over
`Document<()>`, and the embedded `packages/bobcat-element` runtime exposes
the Lynx Element PAPI over them. `LynxView::execute_script(url)` fetches source
through the injected resource contract; the optional QuickJS factory or an
external VM factory runs it against that composition.
What that covers, and what it does not:

**Landed**

- Lynx page defaults (`display: linear`, border-box, hidden overflow) are
  installed as a UA stylesheet, under the `defaultDisplayLinear` and
  `defaultOverflowVisible` page-config switches;
- view metrics and touch-first device construction (`Viewport::device`);
- Lynx element identity as the DOM `NodeId`, with no separate id space; the
  native host boundary validates live IDs and mutation preconditions so
  script misuse surfaces as a JavaScript exception before entering `dom`;
- plain JavaScript handle objects minted by the Element PAPI runtime, with
  `FinalizationRegistry` collection as the one release path, freeing
  exactly one DOM node per handle; descendants remain live as detached
  subtrees until their own handles are collected;
- every ReactLynx Snapshot constructor except `__CreateFrame`, all six tree
  mutation calls (`__AppendElement`, `__InsertElementBefore`, `__RemoveElement`,
  `__ReplaceElement`, `__ReplaceElements`, `__SwapElement`),
  `__FlushElementTree`, and web-core's
  boot sequence. `__CreateList` creates the element but does not yet retain or
  execute its JavaScript callbacks;
- the property surface a Snapshot writes through — `__SetClasses`, `__SetID`,
  `__SetAttribute`, `__SetInlineStyles`, `__AddEvent` — and the queries that
  read it back (`__GetID`, `__GetTag`, `__GetElementUniqueID`, `__GetEvent`,
  `__GetEvents`). Classes, ids, attributes, and inline styles reach stylo
  through the ordinary `Document` setters, so they cascade and lay out on the
  next flush. A string-valued inline style uses the whole-attribute setter;
  a record is cleared and fanned out in JavaScript into name-based
  `set_inline_style_property` calls. This is the name/value subset of CSSOM
  `setProperty` (no priority argument), with no numeric style-id ABI;
- VM-neutral Element-PAPI boot: embedders inject the public
  `ScriptEngineFactory` / `ScriptEngine` host-function protocol; the private
  `MainThreadRuntime` installs the callbacks and performs the same boot for
  QuickJS and external/browser factories;

- `.web.bundle` `StyleInfo` ingestion: a host lowers decoded CSS into
  `bobcat_core::style::PreparsedStyleSheet` and loads it through
  `LynxView::load_style_sheet`, which mounts it as author-origin rules built
  directly — no stylesheet text is produced and no sheet is re-tokenized. The
  CSS parser still owns one selector-list parse per rule and one value parse
  per declaration, because the wire format keeps attribute selectors and
  functional pseudo-classes as text and stylo builds specified values only
  through its value parsers;

**Still open**

- per-component CSS scoping. `StyleInfo` ingestion lands without it: every
  fragment's rules mount globally, which is exactly web-core's own output for
  a bundle compiled with `enableRemoveCSSScope = true` (css id `0`), and the
  CLI warns when a bundle carries non-zero fragment ids. `__SetCSSId` stays
  absent from the PAPI surface until the guard synthesis
  (`:where([l-css-id="N"])`) that gives it meaning exists, together with its
  parent-component css-id inheritance;
- viewport-relative `rpx`/`ppx` units have no owner;
- event *dispatch*, the consuming half of the one member that only records:
  `__AddEvent` stores handlers in the realm with nothing routing input to them
  (no phase walk, no gesture arena);
- the remaining PAPI members (`__AddClass`, `__AddInlineStyle`, the dataset,
  component-info, config, template-part, animation, and selector-query
  members) have no adapter;

These remain runtime-layer responsibilities, divided between
`packages/bobcat-element` policy and `bobcat-core` composition rather than
absorbed into `dom` or `hughie`.

## Invariants

- Every `Document` owns one complete, private style context.
- Snapshot-before-mutate remains internal to document setters.
- Selector matching reads only real DOM state, never opaque payload fields.
- A successful flush harvests and clears all traversal state it consumed.
- Relayout damage is converted to layout invalidation during the internal
  harvest, before the style commit proceeds.
- Standard CSS behavior belongs in `dom`; Lynx-only runtime policy belongs
  above it or in the maintained Stylo fork when it is grammar/value behavior.
- Element handles wrap raw `NodeId`s in JavaScript; the private host boundary
  validates live IDs and tree-mutation preconditions before entering `dom`,
  and reports stale or fabricated IDs as JavaScript errors.
- `PaintOrder` and Painter stay inside `dom`; the element layer has no
  default scene/image/render delegation and builds input frames internally.
- `dom::render` names no DOM vocabulary: no `Document`, `NodeId`, computed
  styles, layout, or paint order.

## Validation

- Core tests: `cargo test -p dom`
- Element-layer tests: `cargo test -p bobcat-core` and
  `pnpm --filter bobcat-element test`
- Runtime feature checks: `cargo check -p bobcat-core --no-default-features`
  and `cargo check -p bobcat-core --features quickjs`
- Core benchmarks: `cargo bench -p dom --bench css` and
  `cargo bench -p dom --bench paint`
- Workspace checks: `cargo fmt --check`, `cargo clippy --all-targets`, and
  `cargo test --workspace`
