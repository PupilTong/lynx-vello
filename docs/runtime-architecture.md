# Runtime architecture

The runtime keeps Lynx policy above the generic DOM and keeps JavaScript
selection at the Bobcat boundary. Painting is different: it is one concrete
document subsystem, not an implementation injected by an embedder.

The product dependency graph is:

```text
bobcat-cli ──▶ bobcat-core ──▶ lynx-element ──▶ dom ─┬─▶ hughie
  │                 │                                  ├─▶ vendor/stylo
  │                 └──▶ quickjs-rust-bridge           └─▶ vello/wgpu
  │                     [feature = "quickjs"]
  ├── lynx-template-decoder
  └── winit (macOS headed product only)

Each layer depends only on the layer directly below and re-exports it whole
(`bobcat_core::lynx_element`, `lynx_element::dom`); `dom` re-exports the
`vello`, `stylo`, `euclid`, and `stylo_traits` vocabulary crates.
```

`dom`'s `render` module is its intentionally DOM-free floor (absorbed from the
former `pulsar` crate): it owns the opaque image registry, the Vello
version boundary (the crate-root `dom::vello` re-export), and the GPU
submission/readback backend. Nothing in it names `Document`, `NodeId`,
computed styles, layout, or paint order; the document-aware painter above it
builds scenes, and the floor turns scenes into pixels.

## Core feature boundary

`bobcat-core` combines engine-neutral protocols and an optional built-in
JavaScript adapter:

- `script::ScriptEngine` is the external JavaScript-engine contract. Its
  `ImportFuture<'a>` is a GAT, so implementations return concrete futures
  without boxed `dyn Future` values.
- `lynx-element` owns the concrete validated Element-PAPI operations and
  `pub type ElementId = u32`. There is no element-host trait: the only real
  host is `ElementTree`, and every PAPI call mutates it directly — the
  tree's own validation is the single source of every `PapiError`.
  `has_uncommitted_mutations` marks the span between a batch's first
  mutation and its `flush_element_tree`, which is what lets a frame
  producer sharing the tree across threads refuse to build from a
  half-applied batch.
- `engine` is the embedder boundary: `Engine` shares the element tree with
  its own Lynx main thread behind one lock, and owns input routing, frame
  production, presentation, and the script thread. An embedder provides
  exactly five things — user input, device metrics, OS initialization, a
  draw target, and IO primitives — and relays OS facts into the engine
  (`dispatch_input`, `resize`, `notify_redraw`, `pump`, clock ticks); it
  never starts or steers the pipeline. The engine schedules through the
  `engine::Window` it borrows at attach time — one trait carrying the draw
  target, the detachable `FrameRequester` its Lynx main thread keeps, and
  `pre_present`. `Engine` is generic over it, so, as with
  `ScriptEngine::ImportFuture`, the boundary needs no boxed closure and no
  `dyn` call: `Window::Target<'window>` is a GAT, so the engine's surface
  borrows the embedder's window rather than demanding a `'static`
  refcounted handle. `engine::OffscreenEngine` is the windowless
  composition, over the uninhabited `NoWindow`.
- `resource` and `view` provide resource acquisition and generic engine/view
  composition. The crate root does not re-export `ElementTree`, `dom`,
  or a renderer specialization. `bobcat-cli` is one embedder of `engine`,
  not the implementation of a core façade.

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
        └── ImageStore
```

`Document::render` performs layout, builds the private CSS visual
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

`lynx_element::ElementTree` directly owns `Document<ElementId>`, and neither
the document nor `NodeId` appears in its public signatures: external
observation speaks `ElementId` only (`page`, `element`, `config`), and DOM
shape is asserted by the layer that owns it — lynx-element's own unit tests,
through a `cfg(test)`-gated accessor. Its mutable surface is the Element
PAPI plus the invariant-safe engine-side methods (`handle_input`,
`set_viewport`, `render`, `needs_render`, `scene`, `images_mut` — none
creates, moves, or retires an element). `bobcat_core::engine::Engine` is the
sole production driver of that engine-side surface, and embedders never
touch it: they hold an `Engine` and relay OS facts into it.

## Frame walkthrough

1. `ElementTree::new` constructs the Lynx `Device`, creates
   `Document<ElementId>`, and installs the Lynx UA stylesheet. The DOM payload
   is the same permanent `u32` id stored in the element arena; private DOM
   `NodeId` slots may still be reused.
2. With QuickJS enabled, `quickjs::MainThreadRuntime` owns only the realm.
   A batch's first Element PAPI mutation takes the tree out of its hand-off
   slot; every call after that is a plain `&mut` mutation with no
   synchronization — the tree validates, so a bad handle throws at the call
   site — without the script ever seeing `NodeId`.
3. `__FlushElementTree` is the commit boundary: the style + layout commit
   runs on the taken tree, the tree goes back in its slot, and the
   presenting side is asked for a frame. The document-owned Painter decides
   whether its retained scene is current.
4. For a dirty frame, `render` flushes/layouts, creates its
   temporary visual order, and runs its private
   Painter over live styles, rounded layouts, retained text, and the
   document-owned `ImageStore`.
5. The Painter resets and rebuilds its retained Vello scene. Headed and
   headless CLI backends borrow that same scene and submit it through
   `dom::render::gpu`; neither backend duplicates DOM traversal or paint
   policy.
6. `Document::handle_input` and the `elements_from_point*` queries are pure
   reads of the visual model the last render retained — hit testing never
   re-runs the pipeline, so events target what the window actually showed.
   `handle_input` performs the resolved default action; scrolling invalidates
   the retained scene, so the next prepared frame rebuilds both it and the
   frame the next event reads.
7. A screenshot reads back the live scene through the mandatory GPU path.
   There is no no-adapter fallback in local tests or CI, and replaced content
   necessarily comes from the document's own image registry.

## Thread topology

The engine libraries are synchronous; the `engine` module passes the one
element tree between exactly two threads through a hand-off slot
(`SharedTree`) — one holder at any instant. The windowed composition:

```text
Lynx main thread (engine-owned)           embedder's event loop thread
QuickJS realm + its event loop            Engine: input routing, scrolling,
batch start = take the tree from slot,    frame production (build + encode),
PAPI calls = plain &mut, zero locks,      GPU submission, present — vsync
flush = commit, put back, ask for ──────▶ interacts with the OS only here
a frame (locks touched 2× per batch)      (non-blocking borrows only)
```

The presenting side borrows the tree from the slot non-blockingly: an
empty slot (a batch is open) means re-present the retained target, buffer
the input, and retry next frame; present's vsync wait always happens
outside the borrow. The slot is occupied while the script merely computes,
so a long JavaScript task between batches never stops input routing,
scrolling, or presentation — scroll target resolution reads the retained
paint order and writes offsets into the borrowed tree, keeping one truth
with no reconciliation protocol. A half-applied batch is unobservable by
construction (the tree is simply absent), and `has_uncommitted_mutations`
guards the one edge where an abandoned batch comes back uncommitted at
the end of an evaluation. The law: the main thread waits only on its own
batch boundaries; the presenting side never waits on the main thread; the
embedder loop never blocks. The offscreen composition keeps the wall-free form: one
thread, `Engine::run_script`, identical semantics — the golden screenshot
suite drives it end to end.

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
cargo check -p dom --all-targets
cargo check -p lynx-element
cargo check -p bobcat-core --no-default-features
cargo check -p bobcat-core --features quickjs
cargo check -p bobcat-cli
cargo check --workspace --all-targets
```

The DOM target
check compiles the private painter, the DOM-free `render` floor, and the paint
tests and benchmark.
The two core builds validate external-engine and built-in QuickJS boundaries;
the final commands validate the product composition.
