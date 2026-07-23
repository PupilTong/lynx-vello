# neutron-star

A trait-first, statically-dispatched CSS **flexbox**, CSS **Grid**, and
Starlight **Linear**/**relative-layout** engine for host-owned trees. Built as
the from-scratch successor to the Lynx C++ engine's `starlight` layout engine.
The host hands the engine stylo-style `Copy` **node handles**
(`LayoutNode`) borrowed from its tree — per-node layout/cache slots are
host-owned interior-mutable state written through the handle — and the style
traits speak the **stylo fork's computed-value vocabulary directly**
(`stylo` with the `lynx` feature is a required dependency, and its build
script needs `python3`; the crate is no longer standalone-publishable).

> **Status: Flexbox, Grid, Linear, Relative Level 1, and text measurement
> implemented.** The
> protocol, shared layout machinery, leaf/absolute sizing, cache, rounding,
> CSS Flexbox Level 1, numeric CSS Grid Level 2, Starlight `display: linear`,
> Starlight Relative Layout Level 1, and the concrete Parley shaping and
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
demands of its DOM. Through the handle the engine reads topology and borrowed
computed-style views (one `Style` associated type; entry points narrow it
with per-algorithm bounds) — all immutable for the
flush — and writes unrounded/final layouts, static positions, and cache
slots into host-owned **interior-mutable per-node slots**. `calc()` needs no
protocol plumbing: stylo's `LengthPercentage` carries and resolves it
itself. There is no
`&mut` anywhere in the protocol, so borrowed style/track views trivially
stay live across recursive layout.
Recursion flows *through the host*: the engine calls
`child.compute_layout(input)`, and the host's impl dispatches each
child to the right algorithm. Flex, Grid, and Lynx's non-CSS Linear and
Relative modes are all first-class neutron-star entry points; a host can
still add other container algorithms through the same dispatch seam.
Leaf content is closed rather than extensible: replaced content enters as a
`NaturalSize`, and the concrete Parley path accepts host-owned
run/style views while the host stores a reusable `TextContext` and per-node
`TextLayoutStore` in interior-mutable slots.
`display:none` cleanup is an explicit host precheck: call `hide_subtree` and
return `LayoutOutput::HIDDEN` before entering the generated-box cache/dispatch
path.

## Hard rules

- **No `dyn`.** Every host boundary is generic. `LayoutNode` is
  dyn-incompatible by construction (`Copy` supertrait, associated types), so
  trait objects are impossible and every call can inline. There is no public
  leaf-measurer trait at all.
- **No storage.** The engine allocates only transient algorithm scratch; node
  data, styles, caches, retained text layouts, and results all live in
  host-chosen storage reached through the handle. Semantic data is immutable
  for a layout epoch; per-node results and caches mutate through the handle
  behind the host's interior-mutability discipline.
- **POD box protocol, closed leaf content.** Layout inputs, outputs, geometry,
  and `NaturalSize` are small `Copy`, `#[repr(C)]` where layout matters, and
  `f32`-based. Images provide decoded dimensions/ratio; concrete Parley text
  retains its rich layout artifact for painting. Arbitrary host content has
  no measurement extension point.
- **Fork-initial defaults.** Defaulted trait methods return the lynx stylo
  fork's initial values — the CSS initial value except where Lynx documents
  otherwise (e.g. `relative-layout-once: true`). Host *computed-value*
  policy (e.g. Lynx computing `box-sizing: auto` to `border-box`, or
  `overflow` to `hidden`) stays the host style system's job.

## Dependencies

The Flex, Grid, Linear, Relative, and text paths are unconditional and require
the workspace's `stylo` fork plus Parley in every configuration (building
stylo needs the vendored submodule and `python3`; a cold build takes minutes).
There is no box-layout-only build: keeping one would make the closed leaf model
and its host behavior configuration-dependent.

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
