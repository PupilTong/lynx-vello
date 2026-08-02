# Runtime architecture

The runtime keeps Lynx policy above the generic DOM and keeps JavaScript
selection at the Bobcat boundary. Painting is different: it is one concrete
document subsystem, not an implementation injected by an embedder.

The product dependency graph is:

```text
bobcat-cli
  ├── lynx-template-decoder
  ├── bobcat-core [feature = "quickjs"] ──▶ quickjs-rust-bridge
  ├── lynx-element [feature = "internal-document-access"]
  │     └──▶ dom ───┬──▶ hughie
  │                 ├──▶ vendor/stylo
  │                 └──▶ pulsar ───▶ vello/wgpu
  ├── pulsar
  └── winit (macOS headed product only)
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
  composition. The crate root does not re-export `ElementTree`, `dom`, Pulsar,
  or a renderer specialization. `bobcat-cli` is a separate product, not the
  implementation of a core embedder façade.

The default `quickjs` feature adds the internal QuickJS implementation,
`QuickJsLynxView`, and the concrete `quickjs::MainThreadRuntime`. QuickJS-only
types have this single module path; the crate root does not duplicate their
exports. Depending on
`bobcat-core` with `default-features = false` excludes the QuickJS adapter and
native QuickJS build while retaining the external engine protocols and element
dependency. `lynx-element` has no QuickJS feature.

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

`Document::render_if_needed` performs layout, builds the private CSS visual
order, and rebuilds the retained scene only when it is stale. The Painter
records the private mutation epoch represented by that scene, so it and
`needs_render` schedule reuse without exposing the epoch to a host.
`Document::scene` lends a guarded shared borrow of the finished scene, and
`Document::images_mut` is the narrow resource-update seam that invalidates it
conservatively. Neither `Painter`, the epoch, nor the private paint order is
public. The old paint-only `paint_style`/`text_layout` reads and the entire
`visual` module are crate-private; only the generic geometry types used by
public input/scroll signatures are re-exported from the `dom` crate root.

This ownership removes two invalid states the injected design permitted:

- a document and renderer could disagree about which image store belonged to
  a frame;
- callers could retain a paint-order snapshot and combine it with newer live
  styles or layout.

`lynx_element::ElementTree` directly owns `Document<ElementId>` but its default
API exposes neither that document nor render/freshness/scene/image forwarding
methods. The trusted CLI and render tests opt into
`internal-document-access`; `bobcat-core` adds no wrapper object or alias.

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
   layout. The CLI-private `FramePipeline` uses its internal document access;
   the document-owned Painter decides whether its retained scene is current.
4. For a dirty frame, `render_if_needed` flushes/layouts, creates its
   temporary visual order, and runs its private
   Painter over live styles, rounded layouts, retained text, and the
   document-owned `ImageStore`.
5. The Painter resets and rebuilds its retained Vello scene. Headed and
   headless CLI backends borrow that same scene and submit it through Pulsar's
   GPU helpers; neither backend duplicates DOM traversal or paint policy.
6. `Document::handle_input` builds the same private visual model for hit
   testing and performs the resolved default action. Scrolling invalidates the
   retained scene, so the next prepared frame rebuilds it.
7. A screenshot reads back the live scene through the mandatory GPU path.
   There is no no-adapter fallback in local tests or CI, and replaced content
   necessarily comes from the document's own image registry.

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
cargo check -p bobcat-cli
cargo check --workspace --all-targets
```

The first two commands verify Pulsar cannot acquire a DOM edge. The DOM target
check compiles the private painter plus its migrated paint tests and benchmark.
The two core builds validate external-engine and built-in QuickJS boundaries;
the final commands validate the product composition.
