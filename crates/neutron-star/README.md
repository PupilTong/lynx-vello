# neutron-star

A trait-first, statically-dispatched box-layout engine for host-owned trees.
CSS **flexbox** is implemented; the CSS **Grid** host protocol is reserved for
the next algorithm milestone. Built as the from-scratch successor to the Lynx
C++ engine's `starlight` layout engine, but deliberately Lynx-agnostic: the
crate has zero required dependencies and is designed to be published and used
standalone.

> **Status: flexbox implemented (milestone L1).** The protocol, shared layout
> machinery, leaf/absolute sizing, cache, rounding, and CSS Flexbox Level 1
> algorithm are implemented. The Grid protocol is present; its layout
> algorithm remains milestone L2. See
> `docs/layout-architecture.md` in the repository root for the full design,
> algorithm plans, milestones, and rationale.

## Design in one paragraph

The engine owns **algorithms and vocabulary**; the host owns **the tree, the
styles, and all storage**. Hosts implement a small family of traits
(`TraverseTree`, `LayoutTree`, `FlexTree`, `GridTree`, `CacheTree`,
`RoundTree`) and per-node style views (`CoreStyle`, `FlexContainerStyle`, …),
then call free generic entry points (`compute_root_layout`,
`compute_leaf_layout`, `compute_flexbox_layout`, …; grid joins in L2).
Recursion flows *through the host*: the engine calls
`LayoutTree::compute_child_layout`, and the host dispatches each child to
the right algorithm — one of neutron-star's, or its own (this is how Lynx's
non-CSS `linear`/`relative` modes plug in as peer algorithms without the engine
knowing about them).

## Hard rules

- **No `dyn`.** Every host boundary is generics + associated types (GATs);
  the traits are structurally not object-safe, so trait objects are impossible
  by construction, and every call across the engine/host boundary can inline.
- **No storage.** The engine allocates only transient algorithm scratch; node
  data, styles, caches, and results all live in host-chosen storage addressed
  by opaque `NodeId`s.
- **Plain-old-data protocol.** Everything crossing the boundary is small,
  `Copy`, `#[repr(C)]` where layout matters, and `f32`-based.
- **CSS-initial defaults.** Trait-method defaults are the CSS initial values;
  host-specific defaults (e.g. Lynx's `box-sizing: border-box` or
  `overflow: hidden`) are the host's job.

## Dependencies and feature flags

None, deliberately. The flex and grid protocols are core, unconditional API,
and the crate compiles with zero dependencies.

## Prior art

The tree/style trait split is informed by [Taffy]'s `LayoutPartialTree`
design (proven to keep a layout engine storage-agnostic without trait
objects), the implemented flex algorithm and planned grid algorithm by the
CSS specs directly (Flexbox Level 1, Grid Level 2, Sizing Level 3), and the
performance posture by `starlight` and [Yoga]. neutron-star shares no code
with any of them.

[Taffy]: https://github.com/DioxusLabs/taffy
[Yoga]: https://github.com/facebook/yoga
