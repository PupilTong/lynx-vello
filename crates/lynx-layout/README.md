# lynx-layout

Host-side Lynx layout protocols and peer algorithms over
[`neutron-star`](../neutron-star).

## Status

The standalone generic `display: linear` milestone is implemented:

- `LinearOrientation`, `LinearGravity`, `LinearCrossGravity`, and
  `LinearLayoutGravity` model the computed Lynx-only value surface;
- `LinearContainerStyle` and `LinearItemStyle` expose allocation-free style
  views;
- `LinearSource` extends neutron-star's immutable `LayoutSource` through
  borrowed GAT views;
- `compute_linear_layout` implements single-axis item sizing and placement,
  positive weighted distribution, alignment and auto margins, baselines,
  hidden children, and linear-aware static positions for out-of-flow
  children.

The algorithm is generic over host storage and uses neutron-star's public
layout IO, `LayoutSession` recursion, cache boundary, box-model support, leaf
measurement seam, and absolute-position machinery. It is a dedicated linear
algorithm, not a translation to CSS flexbox, and the crate currently depends
only on neutron-star.

```rust,ignore
pub fn compute_linear_layout<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    input: LayoutInput,
) -> LayoutOutput
where
    Source: LinearSource,
    Session: LayoutSession<Source>;
```

## Remaining L3 work

This crate is not yet the live runtime bridge. The following remain future
work:

- concrete source/style translation over `lynx-widget` and stylo, including
  `CalcHandle` lowering;
- mutable layout/cache storage, display dispatch, and dirty→cache
  invalidation wiring;
- the root fixed-position pass and sticky post-pass;
- the Lynx-only `display: relative` algorithm;
- Parley-backed `LeafMeasurer` integration and retained text-layout storage.

Those integrations stay here rather than adding Lynx vocabulary or storage
policy to neutron-star.
