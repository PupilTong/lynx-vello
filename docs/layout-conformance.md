# Layout conformance evidence

This document records what neutron-star's executable tests establish. It is a
coverage map, not a claim that the crate implements every feature in the
referenced specifications.

## Reference baselines

- Flex follows the W3C CSS Flexible Box Layout Module Level 1, Candidate
  Recommendation Draft dated 14 October 2025.
- Grid follows the W3C CSS Grid Layout Module Level 2, Candidate
  Recommendation Draft dated 26 March 2025, limited to the numeric track and
  placement surface implemented by neutron-star.
- Linear and Relative follow the non-deprecated Starlight algorithms in
  `lynx-family/lynx` commit
  `e286cd11dda7cc8111d64c2a58d8625bce2bed73`, audited on 14 July 2026.
  The Linear and Relative algorithm headers and implementations were unchanged
  between the repository's local reference checkout and that upstream head.

Normative and source references:

- https://www.w3.org/TR/css-flexbox-1/
- https://www.w3.org/TR/css-grid-2/
- https://github.com/lynx-family/lynx/blob/e286cd11dda7cc8111d64c2a58d8625bce2bed73/core/renderer/starlight/layout/linear_layout_algorithm.cc
- https://github.com/lynx-family/lynx/blob/e286cd11dda7cc8111d64c2a58d8625bce2bed73/core/renderer/starlight/layout/relative_layout_algorithm.cc

## Executable coverage

| Algorithm | Observable behavior covered by native tests | Reference area |
|---|---|---|
| Flex | line collection including exact fits and oversized items; per-line flexible-length resolution; grow, shrink, and freezing; gaps and wrapping; direction and alignment; auto margins; intrinsic measurement and baselines; absolute and hoisted static positions | Flexbox §§4, 5–9 |
| Grid | explicit, automatic, dense, implicit, negative-line, and span placement; fixed, intrinsic, flexible, fit-content, and minmax tracks; item and content alignment; auto margins and baselines; RTL; measurement; absolute grid areas and hoisted static positions | Grid §§7–8, 10–12 |
| Linear | orientation and direction; gravity precedence; weight sums, exhausted space, min/max freezing, and redistribution; order and visibility; intrinsic and constrained measurement; baseline synthesis; auto margins; absolute and hoisted static positions; nested algorithm dispatch | Starlight `DetermineItemSize`, `LayoutWeightedChildren`, `AlignInFlowItems`, `CrossAxisAlignment`, and `SetContainerBaseline` |
| Relative | parent and sibling alignment/adjacency; missing and duplicate ids; deterministic dependency ordering and cycle fallback; one-pass and two-pass measurement; one-sided and double-sided measurement constraints; wrap/minmax feedback; visibility; absolute and hoisted static positions | Starlight `ComputeConstraints`, `GetPositionConstraints`, `Sort`, `LayoutItems`, and `PositionItems` |

The integration suites use the same `LayoutNode` handle and
leaf-measurement protocol as a real host. Every retained case asserts an exact
observable result such as geometry, used margins, baseline, layout order,
static position, measurement input, or cache traffic. Determinism, finite
numbers, source-file contents, and test counts are not correctness oracles.

## Deliberate scope and deviations

The Flex and Grid results only confirm the implemented layout-core surface.
They do not cover CSS parsing, cascade, anonymous box construction, paint,
fragmentation, Grid named areas or named lines, or Grid subgrid. A complete W3C
conformance claim would require the concrete Widget/stylo adapter and the
applicable Web Platform Tests in addition to these engine tests.

Linear and Relative are Lynx-only formatting contexts, so W3C conformance does
not apply to them. Their detailed contracts live in
`starlight-linear-layout.md` and `starlight-relative-layout.md`. Relative keeps
the three previously confirmed repairs documented there: parent id `0` cannot
identify an item, contradictory anchors collapse at start, and two-pass layout
performs selective final-size feedback. These are explicit module semantics;
all other covered behavior is checked against the cited Starlight source.

When any reference baseline changes, update the corresponding behavior test
and this document together. Do not preserve an obsolete result solely because
it appeared in an imported fixture.
