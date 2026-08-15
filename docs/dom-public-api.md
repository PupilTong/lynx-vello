# `dom` public API boundary

This file records the production caller and scope decision for every public
API family in `crates/dom`. Public means an upper crate can name or call it in
a normal build. Implementation types stay crate-private unless a public
cross-crate trait requires otherwise; Stylo's child iterator keeps one
doc-hidden compatibility re-export for its associated type.
An API is retained only when it is either called by production workspace code
or is a complete semantic operation on the generic `Document` boundary. A
diagnostic, cache control, alternate construction path, or partial convenience
operation does not qualify merely because a future adapter might use it.

## Production surface

| API family | Public members | Confirmed production boundary | Why this scope is required |
| --- | --- | --- | --- |
| Core types | `Document`, `Node`, `NodeId` | `bobcat_core::tree::LynxDocument` (= `Document<()>`) is the runtime's document; `flashbulb` accepts documents for capture. | Runtime handles map to document-local ids, while read-only `Node` handles are the document query result. |
| Construction and policy inputs | `Document::new` (takes the permanent document element's tag + payload; `document_element()` returns it non-optionally, `append_document_element` no longer exists), `Device` (dom's own profile struct: `Device::new(width, height, device_pixel_ratio)`, everything else locked — screen, no-quirks, light, coarse touch, fallback font metrics; `standards_device` is the doc-hidden full-parameter test seam), `StylesheetOrigin`, `ElementState`, `Document::{viewport_size,device_pixel_ratio,set_viewport,set_device_pixel_ratio,add_stylesheet}` | `bobcat_core::tree::new_document`, the engine's viewport/device-pixel plumbing, and `flashbulb::frame_size`. | The runtime supplies policy, CSS text, and changing view metrics. Arbitrary device closures, caller-owned base URLs, pre-built rule objects, and detached style resolution are absent. |
| Topology and observable DOM state | `root_node`, `root_element`, `create_element`, `create_text_node`, `append_document_element`, `get`, `is_connected`, `is_ancestor`, `insert_before`, `append_child`, `swap_element`, `remove_element`, `drop_element`, `drop_subtree`; all public `Node` read/navigation methods | the `bobcat` host functions use the mutation subset; direct `Document` embedders use the query side. | This is one coherent DOM-subset boundary. Mutation remains document-owned so aligned arenas and invalidation cannot diverge. Removal is split by what it frees, because the embedder owns the payloads: `remove_element` only unlinks, `drop_element` frees exactly one node and leaves its children allocated, `drop_subtree` frees the whole subtree. Internal slot-zero constants, node-kind enums, child-position helpers, and Stylo local-name objects are not exposed. |
| Selector-visible mutation | `set_classes`, `add_class`, `remove_class`, `set_id_attribute`, `set_attribute`, `remove_attribute`, `add_element_state`, `remove_element_state`, `set_text_node_data`, `set_inline_style` | The generic `Document` host boundary; the element layer forwards `set_attribute`, `remove_attribute`, and the `set_classes`/`set_id_attribute`/`set_inline_style` paths `set_attribute` routes into. `add_class`, `remove_class`, and the element-state pair have no Element-PAPI caller yet. | Each method is a complete observable mutation carrying its own snapshot, restyle, and layout invalidation—not a dirty-state control or partial placeholder. The duplicate one-declaration inline-style merger was removed. |
| Selector queries | `Document::{query_selector, query_selector_all, matches, closest}`, `InvalidSelector` | The generic `Document` host boundary; the background thread's `lynx.createSelectorQuery()` needs exactly this selector-resolution half underneath its batched `NodesRef` builder (docs/tracking/js-runtime.md). | The four methods are the complete `ParentNode`/`Element` query operations, answered by the same Stylo matcher the cascade uses. `InvalidSelector` is public because each of these APIs is specified to throw a `SyntaxError` `DOMException`, so a parse failure must stay distinguishable from "no match" up to the script boundary; the parsed selector list and matching internals stay private. |
| Custom elements | `CustomElement` (`observed_attributes`, `constructed`, `connected_callback`, `disconnected_callback` (shared `&Document`), `attribute_changed_callback`), `Document::define`; `:defined` now matches | The generic `Document` host boundary; a `customElements.define` binding in `bobcat-core` is the named future consumer, reached through its `tree` module, which owns the built-in Lynx component handlers. No `impl CustomElement` and no definition table may live in `dom`. | Registering one handler per local name and receiving the lifecycle reactions is the complete operation for this scope — user-agent components, where `define` requires every definition to precede its elements and the standard's upgrade half is therefore absent by contract rather than by omission. Reactions are raised by the *existing* mutation methods and invoked before each of them returns, so — exactly as with eager slot assignment — no drain, upgrade, or "resolve pending reactions" call exists to expose. The registry, definition ids, reaction records, the element state machine, and the per-element definition pointer stay private; `:defined` is the only state the machine publishes, and an "is it upgraded" query would be a diagnostic. `disconnected_callback` alone takes a shared `&Document` — it is the one callback that runs with a free already committed, so read-only is what makes re-attaching, re-parenting into, or freeing the doomed subtree unrepresentable instead of merely refused. |
| Shadow trees | `attach_shadow`, `ShadowRootMode`, `shadow_root`, `shadow_host`, `shadow_root_mode`, `assigned_slot`, `assigned_nodes`, `add_shadow_stylesheet`, `Node::is_shadow_root` | The generic `Document` host boundary; the element layer does not yet forward `attachShadow`, and web-core reaches shadow DOM through the same W3C surface (`x-input`'s template, `lynx-view`'s own root). | Attaching a root, reading the assignment it produced, and scoping a stylesheet to it are the complete W3C operations; everything downstream (flat-tree styling, layout, paint, hit testing) follows from them with no second entry point. Slot assignment is recomputed eagerly inside the mutation that changed it, so no "resolve pending assignment" call exists to expose. Flat-tree accessors, the shadow-links struct, `<slot>`/`part` local names, and the scoped `CascadeData` stay private. |
| Layout commit and resources | `layout`, `register_fonts(FontBlob)`, `set_natural_size`, `rounded_layout`; `FontBlob`; `layout::{NaturalSize,Size,Layout}` | `bobcat.flushElementTree` (= `Document::layout`) and the engine's `register_fonts` are live. `FontBlob` moves an owned byte container into Parley's retained resource handle without copying its payload; `copy_from_slice` is the explicit fallback for non-owning callers. Natural size is the intentionally narrow `image`→`dom` boundary; rounded layout is consumed by runtime/product integration tests pending more PAPI. | Only owned font input and device-rounded durable geometry cross the boundary. A borrowed font slice cannot outlive registration without a copy. Unrounded geometry, cache contents, full-cache invalidation, containment views, and text artifacts are implementation state. |
| Input and scrolling | `input` types/constructors plus `handle_input`; `scroll` types and document scrolling methods; `Point2D`, `Size2D`, `Vector2D` | the engine's input relay and the macOS Bobcat host; the document default action itself uses the scrolling methods. | Native/headless hosts normalize input and may override default scrolling. Geometry re-exports occur in these signatures. Gesture thresholds and event-validation helpers are private policy. |
| Retained visual output | `elements_from_point`, `elements_from_points`, `render`, `needs_render`, `scene`, `images_mut` | `bobcat_core::engine::Engine` and `flashbulb` schedule and submit the retained scene; the engine's image seam populates images. | `render` rebuilds only a dirty retained scene and is the sole frame producer; the `elements_from_point*` hit queries are `&self` pure reads of the frame it retained (empty before the first render, and fail closed after node removals until the next one). Forced repaint, painter/order types, and visual epochs are not public; the DOM-free `render` module (GPU backend + `ImageStore`) and the `dom::vello` re-export are the public render floor. |

`Node`'s public methods are read-only observations: identity/kind predicates,
parent/child ids, tag/id/class/attribute/state/text/payload/computed style, and
parent/child/sibling iterators. The iterator implementation types stay hidden;
`children()` returns `impl ExactSizeIterator`.

Rustdoc also shows the methods of the Stylo and `selectors` traits implemented
for `Node`. Those implementations are the required zero-copy engine
integration on the same node type, not a second set of inherent forwarding
methods. Their associated iterator/style-view implementation types have the
minimum visibility Rust permits. Hughie's style view stays crate-private;
Stylo's iterator is doc-hidden at the crate root because the public
`TElement` implementation names it.

## Test and benchmark surface

Normal builds contain none of these entry points:

- `layout-test-utils`: synthetic leaf metrics and explicit per-node layout
  invalidation for Hughie protocol benchmarks.

Internal damage and cache probes use `#[cfg(test)]` and never cross the crate
boundary. Integration tests and benchmarks use production commit paths and
observable state—computed style, rounded layout, scroll offsets, hit targets,
retained scene encoding, or GPU pixels.

## Removed surface

The audit removed four kinds of API that had no production contract:

- alternate CSS internals (`with_url_data`, direct rule builders/appenders,
  detached `resolve_style`, media-list wrappers, property/font/keyframe
  inspection, and arbitrary device-update closures);
- standalone flush status/parallelism and normally visible damage records;
- layout identity/cache plumbing (`mark_replaced`, the natural-size getter,
  unrounded geometry, cache probes, and public invalidation controls);
- forced paint, broad renderer dependency re-exports, slot-zero/node-kind/
  local-name details, and duplicate inline-declaration mutation helpers.

Complete CSS text, document mutations, `layout`, retained-scene scheduling,
and observable query results replace those paths.
