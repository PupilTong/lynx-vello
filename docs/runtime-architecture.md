# Runtime architecture

The runtime keeps Lynx policy above the generic DOM and keeps JavaScript
selection at the Bobcat boundary. Painting is different: it is one concrete
document subsystem, not an implementation injected by an embedder.

The product dependency graph is:

```text
bobcat-cli
  ├── lynx-template-decoder
  └── bobcat-core  [feature = "quickjs"]
        ├── lynx-element ───▶ dom ───┬──▶ hughie
        │                            ├──▶ vendor/stylo
        │                            └──▶ pulsar ───▶ vello/wgpu
        └── quickjs-rust-bridge      (only with feature = "quickjs")
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
- `lynx_element::ElementPapi` is the statically dispatched Lynx host contract.
  The protocol and `pub type ElementId = u32` remain owned by
  `lynx-element`; core only re-exports them.
- `resource` and `view` provide resource acquisition and generic engine/view
  composition. `document` re-exports the concrete `Document<T>` through
  `lynx-element` and re-exports `ElementTree`; it performs no renderer
  specialization and adds no direct `dom` dependency.

The default `quickjs` feature adds the internal QuickJS implementation,
`QuickJsLynxView`, and `MainThreadRuntime<H: ElementPapi>`. Depending on
`bobcat-core` with `default-features = false` excludes the QuickJS adapter and
native QuickJS build while retaining the external engine protocols, DOM, and
element layer. `lynx-element` has no QuickJS feature.

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
rebuilds the retained scene. `Document::scene` lends a guarded shared borrow of
that finished scene. `Document::images_mut` is the narrow resource-update seam
and advances `visual_epoch` conservatively. Neither `Painter` nor the private
paint order is public.

This ownership removes two invalid states the injected design permitted:

- a document and renderer could disagree about which image store belonged to
  a frame;
- callers could retain a paint-order snapshot and combine it with newer live
  styles or layout.

`lynx_element::ElementTree` directly owns `Document<ElementId>` and delegates
`render`, `scene`, `images`, and `images_mut` without lending out
`&mut Document`. `bobcat-core` adds no wrapper object; its document module is a
type alias/re-export layer.

## Frame walkthrough

1. `ElementTree::new` constructs the Lynx `Device`, creates
   `Document<ElementId>`, and installs the Lynx UA stylesheet. The DOM payload
   is the same permanent `u32` id stored in the element arena; private DOM
   `NodeId` slots may still be reused.
2. With QuickJS enabled, `MainThreadRuntime<ElementTree>` installs the five
   supported Element PAPI host functions through `ElementPapi`. Script mutates
   the validated element layer without seeing `NodeId` or mutable DOM access.
3. `__FlushElementTree` attaches `<page>` on first use and commits style and
   layout. `FramePipeline` watches `Document::visual_epoch` and skips a static
   scene.
4. For a dirty frame, `ElementTree::render` calls `Document::render`. DOM
   flushes/layouts, creates its temporary visual order, and runs its private
   Painter over live styles, rounded layouts, retained text, and the
   document-owned `ImageStore`.
5. The Painter resets and rebuilds its retained Vello scene. Headed and
   headless CLI backends borrow that same scene and submit it through Pulsar's
   GPU helpers; neither backend duplicates DOM traversal or paint policy.
6. `Document::handle_input` builds the same private visual model for hit
   testing and performs the resolved default action. Scrolling advances
   `visual_epoch`, so the next prepared frame rebuilds the scene.
7. A screenshot reads back the live scene through the mandatory GPU path.
   There is no no-adapter fallback in local tests or CI, and replaced content
   necessarily comes from the document's own image registry.

## Static dispatch and intentional dynamic boundaries

The JavaScript protocol, Element PAPI host, DOM payload, and Hughie layout host
use static dispatch. Painting is concrete document behavior, so it needs no
trait dispatch at all. Dynamic dispatch remains only where heterogeneity is
the protocol requirement: resource fetcher handles, async byte readers, font
providers inherited from Stylo, and resource-path leases. Those objects do not
enter layout or scene traversal.

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
