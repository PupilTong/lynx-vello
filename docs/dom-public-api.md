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
| Core types | `Document`, `Node`, `NodeId` | `lynx-element::ElementTree` owns `Document<ElementId>`; `flashbulb` accepts documents for capture. | Runtime handles map to document-local ids, while read-only `Node` handles are the document query result. |
| Construction and policy inputs | `Document::new`, `Device`, `StylesheetOrigin`, `ElementState`, `Document::{device,set_viewport,set_device_pixel_ratio,add_stylesheet}` | `ElementTree::{new,set_viewport,set_device_pixel_ratio,add_author_stylesheet}` and `flashbulb::frame_size`. | The runtime supplies policy, CSS text, and changing view metrics. Arbitrary device closures, caller-owned base URLs, pre-built rule objects, and detached style resolution are absent. |
| Topology and observable DOM state | `root_node`, `root_element`, `create_element`, `create_text_node`, `append_document_element`, `get`, `is_connected`, `is_ancestor`, `insert_before`, `append_child`, `detach`, `remove_subtree`; all public `Node` read/navigation methods | `ElementTree::{create_page,create_view,append_element,drop_element,flush_element_tree}` uses the mutation subset; direct `Document` embedders use the query side. | This is one coherent DOM-subset boundary. Mutation remains document-owned so aligned arenas and invalidation cannot diverge. Internal slot-zero constants, node-kind enums, child-position helpers, and Stylo local-name objects are not exposed. |
| Selector-visible mutation | `set_classes`, `add_class`, `remove_class`, `set_id_attribute`, `set_attribute`, `remove_attribute`, `add_element_state`, `remove_element_state`, `set_text_node_data`, `set_inline_style` | The generic `Document` host boundary; the remaining Element-PAPI forwarding is not yet implemented in `lynx-element`. | Each method is a complete observable mutation carrying its own snapshot, restyle, and layout invalidation—not a dirty-state control or partial placeholder. The duplicate one-declaration inline-style merger was removed. |
| Layout commit and resources | `layout`, `register_fonts`, `set_natural_size`, `rounded_layout`; `layout::{NaturalSize,Size,Layout}` | `ElementTree::{flush_element_tree,register_fonts}` is live. Natural size is the intentionally narrow `image`→`dom` boundary; rounded layout is consumed by runtime/product integration tests pending more PAPI. | Only device-rounded durable geometry crosses the boundary. Unrounded geometry, cache contents, full-cache invalidation, containment views, and text artifacts are implementation state. |
| Input and scrolling | `input` types/constructors plus `handle_input`; `scroll` types and document scrolling methods; `Point2D`, `Size2D`, `Vector2D` | `ElementTree::handle_input` and the macOS Bobcat host; the document default action itself uses the scrolling methods. | Native/headless hosts normalize input and may override default scrolling. Geometry re-exports occur in these signatures. Gesture thresholds and event-validation helpers are private policy. |
| Retained visual output | `hit_test`, `render`, `needs_render`, `scene`, `images_mut` | `bobcat-cli::FramePipeline` and `flashbulb` schedule and submit the retained scene; Bobcat screenshot integration populates images. | `render` rebuilds only a dirty retained scene. Forced repaint, painter/order types, and visual epochs are not public; the DOM-free `render` module (GPU backend + `ImageStore`) and the `dom::vello` re-export are the public render floor. |

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
