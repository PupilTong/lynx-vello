# neutron-star

A trait-first, statically-dispatched CSS **flexbox**, CSS **Grid**, and
Starlight **Linear**/**relative-layout** engine for host-owned trees. Built as
the from-scratch successor to the Lynx C++ engine's `starlight` layout engine.
It is host- and storage-agnostic despite exposing Lynx-specific algorithms:
its protocol and box-layout core have zero dependencies when default features
are disabled, and the crate is designed to be published and used standalone.

> **Status: Flexbox, Grid, Linear, Relative Level 1, and text measurement
> implemented.** The
> protocol, shared layout machinery, leaf/absolute sizing, cache, rounding,
> CSS Flexbox Level 1, numeric CSS Grid Level 2, Starlight `display: linear`,
> Starlight Relative Layout Level 1, and the feature-gated Parley shaping and
> line-breaking core are implemented. Grid
> deliberately excludes subgrid and
> host-lowered named lines/areas. See
> `docs/layout-architecture.md` in the repository root for the full design,
> algorithm plans, milestones, and rationale.

## Design in one paragraph

The engine owns **algorithms and vocabulary**; the host owns **the tree, the
styles, and all storage**. The protocol is one trait: `LayoutNode`, a cheap
`Copy` **node handle** borrowed from the host's tree for one layout flush —
a plain `&'dom Node` or a `(&'dom Tree, index)` pair, the same shape stylo
demands of its DOM. Through the handle the engine reads topology, borrowed
computed-style views (one `Style` associated type; entry points narrow it
with per-algorithm bounds), and `calc()` resolution — all immutable for the
flush — and writes unrounded/final layouts, static positions, and cache
slots into host-owned **interior-mutable per-node slots**. There is no
`&mut` anywhere in the protocol, so borrowed style/track views trivially
stay live across recursive layout.
Recursion flows *through the host*: the engine calls
`child.compute_child_layout(input)`, and the host's impl dispatches each
child to the right algorithm. Flex, Grid, and Lynx's non-CSS Linear and
Relative modes are all first-class neutron-star entry points; a host can
still add other modes through the same dispatch seam.
The optional text adapter uses the same seam: host-owned run/style views are
immutable, while the host stores a reusable `TextContext` and per-node
`ArtifactSlots` in interior-mutable slots.
`display:none` cleanup is an explicit host precheck: call `hide_subtree` and
return `LayoutOutput::HIDDEN` before entering the generated-box cache/dispatch
path.

## Hard rules

- **No `dyn`.** Every host boundary is generic. `LayoutNode` is
  dyn-incompatible by construction (`Copy` supertrait, associated types) and
  the measurement seam uses GATs, so trait objects are impossible and every
  call can inline.
- **No storage.** The engine allocates only transient algorithm scratch; node
  data, styles, caches, retained text layouts, and results all live in
  host-chosen storage reached through the handle. Semantic data is immutable
  for a layout epoch; per-node results and caches mutate through the handle
  behind the host's interior-mutability discipline.
- **POD box protocol, lending measurement seam.** Layout inputs, outputs, and
  geometry are small `Copy`, `#[repr(C)]` where layout matters, and
  `f32`-based. `LeafMeasurer` may additionally lend an engine-specific rich
  artifact view; leaf boxing immediately copies its size/baselines into
  `LeafMetrics`, while the host retains the artifact for painting.
- **Specification-owned defaults.** Core/Flex/Grid trait methods use CSS
  initial values; Linear and Relative methods use their Starlight
  specification initial values. Host-specific defaults (e.g. Lynx's
  `box-sizing: border-box`, `overflow: hidden`, or
  `relative-layout-once: true`) are the host's job.

## Dependencies and feature flags

The Flex, Grid, Linear, Relative, and text-style protocols are unconditional,
and `default-features = false` keeps that protocol and box-layout core at zero
dependencies. Default builds enable the `text` feature and its optional
Parley dependency for shaping, line breaking, and retained text measurement.

## Prior art

The `Copy` node-handle protocol mirrors [stylo]'s `TNode`/`TElement` DOM
pattern (handles carry the tree lifetime; per-node mutable state sits in
interior-mutable slots), the host-dispatch recursion is informed by
[Taffy]'s `LayoutPartialTree`
design (proven to keep a layout engine storage-agnostic without trait
objects), the implemented Flex and Grid algorithms by the CSS specs directly
(Flexbox Level 1, Grid Level 2, Sizing Level 3), Linear behavior by
Starlight, Relative by its standalone Starlight implementation contract, and
the performance posture by `starlight` and [Yoga]. neutron-star shares no code
with any of them.

[Taffy]: https://github.com/DioxusLabs/taffy
[Yoga]: https://github.com/facebook/yoga
