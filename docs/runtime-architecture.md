# Runtime architecture

The runtime is split around one rule: Lynx policy lives above the generic DOM,
while the selected JavaScript and rendering implementations are injected
statically. The current product dependency graph is:

```text
bobcat-cli
  ├── lynx-template-decoder
  └── bobcat-core  [feature = "quickjs"]
        ├── lynx-element ───▶ dom ──────▶ hughie + vendor/stylo
        │     └── stylo       (Lynx Device/UA-policy construction)
        ├── pulsar ─────────▶ dom + vello
        └── quickjs-rust-bridge
```

`bobcat-cli` depends on the assembled `bobcat-core` API and enables its
`quickjs` feature. `bobcat-core` directly composes the independent
`lynx-element` policy crate with Pulsar. `lynx-element` depends directly on
`dom` and has no dependency or feature edge back to core, Pulsar, or QuickJS.

## Core feature boundary

`bobcat-core` combines the engine-neutral protocols and optional built-in
JavaScript adapter in one crate. Its always-available surface is
engine-neutral:

- `script::ScriptEngine` is the external JavaScript-engine contract. Its
  `ImportFuture<'a>` is a GAT, so an implementation returns its own concrete
  future type instead of forcing a boxed future or a script-engine trait
  object.
- `lynx-element::ElementPapi` is the statically dispatched Lynx host contract.
  Core re-exports it from `element` and uses it in JavaScript adapters; the
  protocol and `ElementId = u32` remain owned by `lynx-element`.
- `resource`, `view`, and `document` provide resource acquisition, generic
  engine composition, and the rendered document specialization.

The default `quickjs` feature adds the internal QuickJS implementation,
`QuickJsLynxView`, and `MainThreadRuntime<H: ElementPapi>`. Depending on
`bobcat-core` with `default-features = false` excludes both the QuickJS Rust
adapter and the native QuickJS build while preserving the external traits and
rendered element composition. There is no forwarding QuickJS feature on
`lynx-element`; upper layers such as `bobcat-cli` enable `bobcat-core/quickjs`
directly, whereas crates such as `image` do not.

## Renderer injection

The generic DOM type is `dom::Document<T, R = ()>`. `T` remains the opaque
per-node payload; `R` is one renderer chosen at compile time and installed by
`Document::with_renderer` (or `with_url_data_and_renderer`). Plain DOM users
continue to use `Document<T>`, whose renderer is `()`.

`dom::visual::DocumentRenderer<T>` is the render contract:

```rust,ignore
trait DocumentRenderer<T> {
    type Output<'a>
    where
        Self: 'a,
        T: 'a;

    fn render(&mut self, document: &Document<T, Self>, frame: &PaintOrder);
    fn output<'a>(renderer: Ref<'a, Self>) -> Self::Output<'a>;
}
```

The GAT is load-bearing: a renderer can return a borrowed or guarded retained
result without cloning it and without `Box<dyn Renderer>`. Pulsar implements
the trait with `Output<'a> = Ref<'a, vello::Scene>`. Its implementation owns
one reusable `Painter` and one `ImageStore`, so those allocations no longer
need a parallel owner in every embedder.

`lynx_element::ElementTree<R = ()>` owns
`dom::Document<ElementId, R>` and provides `with_renderer` for static
composition. The default `ElementTree` is DOM-only. Bobcat's
`document::ElementTree` facade injects `pulsar::Pulsar` and retains the
Pulsar-specific `scene`/image-store surface in core; `lynx-element` never
imports the renderer. `bobcat_core::document::Document<T>` remains the lower
level `dom::Document<T, Pulsar>` specialization.

The renderer is retained for exactly the document's lifetime.
`Document::renderer_mut` is the resource-update seam; accessing it
conservatively advances `visual_epoch`, because generic DOM cannot inspect
whether a renderer resource changed. Neither element-tree layer exposes a
mutable document.

## Frame walkthrough

1. `bobcat_core::document::ElementTree::new` passes a new Pulsar into
   `lynx_element::ElementTree::with_renderer`. The element layer builds the
   Lynx `Device`, constructs `dom::Document<ElementId, Pulsar>`, and installs
   the Lynx UA stylesheet. `lynx-element` owns `type ElementId = u32`; each DOM
   payload is the same permanent id stored by the element arena.
2. With QuickJS enabled, `MainThreadRuntime<ElementTree>` installs the five
   supported Element PAPI host functions through the generic `ElementPapi`
   contract. A script mutates the element tree without seeing `NodeId` or a
   mutable `Document`.
3. `__FlushElementTree` attaches the page on first use and commits style and
   layout. `FramePipeline` watches `Document::visual_epoch` to avoid rebuilding
   a static scene.
4. When a frame is dirty, the generic `ElementTree::render` calls
   `Document::render`.
   The document creates its private `PaintOrder` and invokes its concrete
   `Pulsar`; `PaintOrder` never crosses the `lynx-element` API.
5. Pulsar rebuilds its retained Vello scene. Bobcat's `ElementTree::scene`
   returns a guarded borrow of that scene through the renderer GAT; headed and
   headless CLI backends submit the same scene without a clone.
6. Input follows the same ownership rule. `ElementTree::handle_input` builds
   the temporary paint order needed for hit testing internally. A scrolling
   default action advances `visual_epoch`, so the next prepared frame refreshes
   the retained scene.

## Static dispatch and intentional dynamic boundaries

The JavaScript engine, Element PAPI host, document renderer, DOM payload, and
layout host are statically dispatched. No `dyn` participates in the frame or
PAPI hot path. Dynamic dispatch remains only where heterogeneity is itself the
protocol requirement: resource fetcher handles, async byte readers, and
resource-path leases. Those objects do not enter style, layout, paint-order,
or scene traversal.

## Validation matrix

```sh
cargo check -p bobcat-core --no-default-features
cargo check -p bobcat-core --features quickjs
cargo check -p lynx-element
cargo check -p bobcat-cli
cargo check --workspace --all-targets
```

The first command is the external-engine build boundary; the second validates
the built-in engine. The third verifies the DOM-only element crate cannot
acquire a runtime or renderer dependency, and the fourth validates the
product composition used by the CLI.
