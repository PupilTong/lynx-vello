# Runtime architecture

The runtime keeps Lynx policy above the generic DOM and keeps JavaScript
selection at the Bobcat boundary. Painting is different: it is one concrete
document subsystem, not an implementation injected by an embedder. Product
embedders enter through `bobcat_core::renderer`; retained scenes, GPU queues,
and frame-freshness predicates do not cross that boundary.

The product dependency graph is:

```text
bobcat-cli
  ├── lynx-template-decoder
  └── bobcat-core  [features = "quickjs", "renderer"]
        ├── lynx-element ───▶ dom ───┬──▶ hughie
        │                            ├──▶ vendor/stylo
        │                            └──▶ pulsar ───▶ vello/wgpu
        ├── quickjs-rust-bridge      (feature = "quickjs")
        └── winit                    (renderer window host; macOS/Linux only)
```

`pulsar` is intentionally below and independent of `dom`: it owns the opaque
image registry, Vello version boundary, and GPU submission/readback backend.
It knows no `Document`, `NodeId`, computed style, layout, or paint order.
`dom` owns every DOM-aware paint operation and uses Pulsar's lower-level
resources directly.

## Core feature boundary

`bobcat-core` combines engine-neutral protocols and an optional built-in
JavaScript adapter:

- `script::ScriptEngine` is the external JavaScript-engine contract. Its
  `ImportFuture<'a>` is a GAT, so implementations return concrete futures
  without boxed `dyn Future` values.
- `lynx-element` owns the concrete validated Element-PAPI operations and
  `pub type ElementId = u32`. There is no element-host trait: the only real
  host is `ElementTree`, so the QuickJS adapter composes it directly.
- `resource` and `view` provide resource acquisition and generic engine/view
  composition. The crate root deliberately does **not** re-export
  `ElementTree`, `dom`, or `pulsar`: those paths expose lower-layer paint and
  GPU vocabulary that is not part of the product embedder contract.
- `renderer` is the product composition layer. `RenderProgram` holds decoded
  main-thread input, `RenderRuntime` boots it for an explicit viewport,
  `HeadlessRenderer` owns its synthetic-vsync clock and retained GPU target,
  and `WindowRenderer` derives the initial viewport/DPR from its native window
  and owns display-vsync surface selection, presentation, and readback on
  macOS/Linux. Its public frame value is only `CapturedFrame` (size plus RGBA
  bytes), produced on explicit capture.
  This feature depends directly on `dom` and `pulsar`; `lynx-element` no longer
  re-exports either layer.

The default `quickjs` feature adds the internal QuickJS implementation,
`QuickJsLynxView`, and the concrete `quickjs::MainThreadRuntime`. QuickJS-only
types have this single module path; the crate root does not duplicate their
exports. Depending on
`bobcat-core` with `default-features = false` excludes the QuickJS adapter and
native QuickJS build while retaining the external engine protocols and element
dependency. The non-default `renderer` feature implies `quickjs`; it adds the
product façade and native window dependency without changing the
engine-neutral protocol build. `lynx-element` has no QuickJS or window feature.

## Document-owned painting

The generic document type is simply `dom::Document<T>`. `T` is the opaque
per-node payload. There is no renderer type parameter, `DocumentRenderer`
trait, `with_renderer` constructor, or renderer/resource escape hatch.

Each document privately owns one reusable paint state:

```text
Document<T>
  ├── DOM + Stylo arenas
  ├── layout/text state
  └── private Painter
        ├── retained vello::Scene
        ├── reusable walk scratch
        └── pulsar::ImageStore
```

`Document::render` performs layout, builds the private CSS visual order, and
rebuilds the retained scene. The Painter records the private mutation epoch
represented by that scene; the lower rendering layers use
`Document::render_if_needed` and `needs_render` to schedule reuse without
exposing the epoch. `Document::scene` lends a guarded shared borrow of the
finished scene inside that composition, and
`Document::images_mut` is the narrow resource-update seam that invalidates it
conservatively. Neither `Painter`, the epoch, nor the private paint order is
public. These are lower-crate implementation/test APIs, not product embedder
APIs: `bobcat_core::renderer` publishes none of their names or types. The old
paint-only `paint_style`/`text_layout` reads and the entire `visual` module are
crate-private; only the generic geometry types used by public input/scroll
signatures are re-exported from the `dom` crate root.

This ownership removes two invalid states the injected design permitted:

- a document and renderer could disagree about which image store belonged to
  a frame;
- callers could retain a paint-order snapshot and combine it with newer live
  styles or layout.

`lynx_element::ElementTree` directly owns `Document<ElementId>`. The internal
composition layer accesses that document directly; `ElementTree` has no
render, freshness, scene, or image-store forwarding methods. The renderer
façade keeps the document and its paint APIs out of product code.

## Frame walkthrough

1. `ElementTree::new` constructs the Lynx `Device`, creates
   `Document<ElementId>`, and installs the Lynx UA stylesheet. The DOM payload
   is the same permanent `u32` id stored in the element arena; private DOM
   `NodeId` slots may still be reused.
2. With QuickJS enabled, `quickjs::MainThreadRuntime` owns one `ElementTree`
   and installs its five supported Element PAPI host functions directly.
   Script mutates the validated element layer without seeing `NodeId` or
   mutable DOM access.
3. `__FlushElementTree` attaches `<page>` on first use and commits style and
   layout. The private `renderer::pipeline` asks the element/document layers
   for a prepared frame; the document-owned Painter decides whether its
   retained scene is current.
4. For a dirty frame, `render_if_needed` calls `Document::render`. DOM
   flushes/layouts, creates its temporary visual order, and runs its private
   Painter over live styles, rounded layouts, retained text, and the
   document-owned `ImageStore`.
5. The Painter resets and rebuilds its retained Vello scene. The renderer
   façade privately borrows that scene: `HeadlessRenderer` submits it to its
   retained target and bounds in-flight work, while `WindowRenderer` submits,
   blits, and presents through an `AutoVsync` surface. The CLI sees neither
   branch and performs no GPU submission.
6. `Document::handle_input` builds the same private visual model for hit
   testing and performs the resolved default action. Scrolling invalidates the
   retained scene, so the next prepared frame rebuilds it.
7. An explicit `capture()` returns `CapturedFrame` after reading the live
   target through the mandatory GPU path. There is no no-adapter fallback in
   local tests or CI, and replaced content necessarily comes from the
   document's own image registry.

## Static dispatch and intentional dynamic boundaries

The JavaScript protocol and Hughie layout host use static dispatch. The Element
PAPI host and painting are concrete runtime/document behavior, so neither has
a trait dispatch boundary. The DOM payload is an ordinary generic parameter.
Dynamic dispatch remains only where heterogeneity is the protocol requirement:
resource fetcher handles, async byte readers, font providers inherited from
Stylo, and resource-path leases. Those objects do not enter layout or scene
traversal.

## Validation matrix

```sh
cargo check -p pulsar
cargo tree -p pulsar --edges normal --depth 1
cargo check -p dom --all-targets
cargo check -p lynx-element
cargo check -p bobcat-core --no-default-features
cargo check -p bobcat-core --features quickjs
cargo check -p bobcat-core --features renderer --all-targets
cargo check -p bobcat-cli
cargo check --workspace --all-targets
```

The first two commands verify Pulsar cannot acquire a DOM edge. The DOM target
check compiles the private painter plus its migrated paint tests and benchmark.
The three core builds validate external-engine, built-in QuickJS, and product
renderer boundaries; the final commands validate the CLI composition. The
window renderer source is shared by macOS and Linux; winit enables both
Wayland and X11 on Linux.
