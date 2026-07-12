# neutron-star

A trait-first, statically-dispatched CSS **flexbox**/**Grid** and Starlight
**relative-layout** engine for host-owned trees. Built as the from-scratch
successor to the Lynx C++ engine's `starlight` layout engine, the crate has
zero required dependencies and is designed to be published and used
standalone.

> **Status: Flex, Grid, and Relative Level 1 implemented.** The protocol,
> shared layout machinery, leaf/absolute sizing, cache, rounding, CSS Flexbox
> Level 1, numeric CSS Grid Level 2, and Starlight Relative Layout Level 1
> algorithms are implemented. Grid deliberately excludes subgrid and
> host-lowered named lines/areas. See
> `docs/layout-architecture.md` in the repository root for the full design,
> algorithm plans, milestones, and rationale.

## Design in one paragraph

The engine owns **algorithms and vocabulary**; the host owns **the tree, the
styles, and all storage**. The protocol deliberately exposes those through
two separate objects: an immutable `LayoutSource` (`TraverseTree`,
`FlexSource`, `GridSource`, `RelativeSource`) containing topology and
computed-style views, and a mutable object implementing `LayoutSession`
(`LayoutState` + `CacheState`)
and, when pixel snapping is used, the independent `RoundState` capability.
That mutable side contains results, caches, measurement resources, and
display dispatch.
Recursion flows *through the host*: the engine calls
`LayoutSession::compute_child_layout(source, …)`, and the host dispatches each
child to the right algorithm — one of neutron-star's, or its own. Relative is
a built-in generic algorithm selected by host dispatch; Lynx's non-CSS
`linear` mode remains an ordinary host-private peer. The split lets each
algorithm retain borrowed style views while recursive layout mutates only the
session.
`display:none` cleanup is an explicit host precheck: call `hide_subtree` and
return `LayoutOutput::HIDDEN` before entering the generated-box cache/dispatch
path.

## Hard rules

- **No `dyn`.** Every host boundary is generic. Source/measurement traits use
  GATs and mutable capability traits explicitly require `Sized`, so trait
  objects are impossible by construction and every call can inline.
- **No storage.** The engine allocates only transient algorithm scratch; node
  data, styles, caches, retained text layouts, and results all live in
  host-chosen storage addressed by opaque `NodeId`s. Semantic source data is
  immutable for a layout epoch; mutable results and caches live separately.
- **POD box protocol, lending measurement seam.** Layout inputs, outputs, and
  geometry are small `Copy`, `#[repr(C)]` where layout matters, and
  `f32`-based. `LeafMeasurer` may additionally lend an engine-specific rich
  artifact view; leaf boxing immediately copies its size/baselines into
  `LeafMetrics`, while the host retains the artifact for painting.
- **Protocol-initial defaults.** CSS trait methods use CSS initial values;
  Relative methods use the standalone Relative Level 1 initial values.
  Host-specific defaults (e.g. Lynx's `box-sizing: border-box`,
  `overflow: hidden`, or `relative-layout-once: true`) are the host's job.

## Dependencies and feature flags

None, deliberately. The Flex, Grid, and Relative protocols are core,
unconditional API, and the crate compiles with zero dependencies.

## Prior art

The source/session/style protocol split is informed by [Taffy]'s `LayoutPartialTree`
design (proven to keep a layout engine storage-agnostic without trait
objects), the implemented Flex and Grid algorithms by the CSS specs directly
(Flexbox Level 1, Grid Level 2, Sizing Level 3), Relative by its standalone
Starlight implementation contract, and the performance posture by
`starlight` and [Yoga]. neutron-star shares no code with any of them.

[Taffy]: https://github.com/DioxusLabs/taffy
[Yoga]: https://github.com/facebook/yoga
