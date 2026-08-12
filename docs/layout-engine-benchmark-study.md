# Flex layout engine benchmark study

Date: 2026-08-12 (Asia/Singapore)

This knowledge-base entry records the conclusions of a one-off local Flexbox
study covering Hughie, Taffy 0.13.0, Yoga 3.2.1, and Lynx Starlight. It does not
make the comparison harness part of the product repository. In particular,
the C++ runners, external-engine Rust adapters, result-processing scripts, and
raw samples are intentionally not checked in.

The repository retains only five standalone Hughie regression cases in the
`hughie_flex` Cargo benchmark target. Those cases use Hughie's production DOM
host and therefore preserve the useful workload shapes without creating a
test-code dependency on another layout engine. They are not a reproduction of
the low-level cross-engine timings below.

## Scope

The study used the strict Flexbox behavior shared by all four engines. It
excluded Grid because Yoga does not implement Grid, and excluded Lynx-only
Linear and Relative layout because Taffy and Yoga have no equivalent. Text,
baselines, positioned descendants, containment, and measurement callbacks
were also excluded because their host contracts are not equivalent.

Five deterministic 4,095-node trees were measured:

| Scenario | Shape and pressure |
| --- | --- |
| `wide_active_grow` | One row root plus 4,094 leaves; 1 px bases, grow factors 1/2/3, and positive free space. |
| `wide_wrap_gap` | A 320 px wrapping row with 1 px gaps and leaf sizes cycling through 16–20 × 6–8 px. |
| `wide_grow_freeze` | 100 px bases with positive free space; alternating leaves capped at 120 px. |
| `wide_shrink_freeze` | 100 px bases with negative free space; alternating minimum widths of 90 and 0 px. |
| `balanced_deep` | An exact-size breadth-first binary tree whose internal nodes are both row Flex containers and Flex items. |

## Findings

- Taffy had the lowest median initial-layout time for all four wide trees.
- Starlight had the lowest median initial-layout time for the balanced deep
  tree.
- Hughie ranked second on all five initial workloads. It was 12.8–14.0% behind
  Taffy on the wide cases and 48.6% behind Starlight on the deep case, while
  remaining ahead of Taffy and Yoga on that deep case.
- Hughie and Starlight were close on the three non-wrapping wide cases: the
  median difference was 1.3% for active growth and below 1% for both freeze
  cases.
- Taffy had the lowest median incremental-layout time in all five cases.
  Hughie ranked second, trailing by 15.7–25.8% on the wide cases and by 2.15×
  on the balanced deep tree.
- Unchanged-tree results were dominated by public-entry and cache policy, so
  they are not a common algorithm ranking. Yoga returned immediately from a
  clean root, while Starlight's standalone entry dirtied the root on every
  call.

An equal-weight geometric mean of each engine's initial-layout ratio to the
fastest engine per scenario was Taffy 1.124×, Starlight 1.134×, Hughie 1.196×,
and Yoga 2.771×. This is a summary of five synthetic cases, not a general
product score.

## Initial-layout medians

Values are milliseconds for one fresh 4,095-node tree. Parentheses show the
ratio to the fastest engine in that row.

| Scenario | Taffy | Hughie | Starlight | Yoga |
| --- | ---: | ---: | ---: | ---: |
| `wide_active_grow` | **0.478 (1.00×)** | 0.539 (1.13×) | 0.546 (1.14×) | 1.242 (2.60×) |
| `wide_wrap_gap` | **0.430 (1.00×)** | 0.491 (1.14×) | 0.542 (1.26×) | 2.393 (5.56×) |
| `wide_grow_freeze` | **0.505 (1.00×)** | 0.572 (1.13×) | 0.578 (1.14×) | 1.210 (2.40×) |
| `wide_shrink_freeze` | **0.512 (1.00×)** | 0.579 (1.13×) | 0.583 (1.14×) | 1.152 (2.25×) |
| `balanced_deep` | 1.243 (1.79×) | 1.032 (1.49×) | **0.694 (1.00×)** | 1.456 (2.10×) |

## Incremental-layout medians

Values are microseconds after changing one unfrozen leaf's numeric Flex basis
by one pixel and invalidating its path to the root. Mutation and invalidation
were outside the measured interval.

| Scenario | Taffy | Hughie | Starlight | Yoga |
| --- | ---: | ---: | ---: | ---: |
| `wide_active_grow` | **430.9** | 542.2 | 562.9 | 1,112.6 |
| `wide_wrap_gap` | **346.9** | 404.3 | 554.0 | 2,064.6 |
| `wide_grow_freeze` | **457.5** | 530.3 | 590.6 | 987.4 |
| `wide_shrink_freeze` | **464.5** | 537.1 | 591.3 | 1,027.7 |
| `balanced_deep` | **31.4** | 67.7 | 124.7 | 75.8 |

## Interpretation for Hughie

The wide Flex core is already close to the best result in this small study.
The clearest optimization targets are the deep-tree initial path and especially
deep incremental invalidation/recomputation. The standalone Hughie cases keep
all five shapes visible in CodSpeed without retaining cross-engine machinery.

Do not compare future `hughie_flex` numbers directly with the tables above.
The committed target intentionally exercises `Document::layout` through the
production host, whereas the local study isolated each engine's lower-level
layout entry. The committed cases are suitable for Hughie before/after
regression tracking; the tables are historical cross-engine evidence.

## Environment and limitations

The retained run used a MacBook Pro `MacBookPro18,2` with an Apple M1 Max
(10 CPU cores, 64 GB), AC power, macOS 26.4.1 arm64, Rust nightly 1.98.0, and
Apple Clang 21.0.0. The engine revisions were Hughie
`a1d6c6c2f9da760bd66daff7272795736fc8eda9`, Taffy 0.13.0, Yoga 3.2.1
`042f5013152eb81c1552dec945b88f7b95ca350f`, and Lynx Starlight
`66b002855a25a5a8812fe878af69e20a346d0408`.

This was one machine and one session. CPU frequency and thermal state were not
pinned, engine order was fixed, and confidence intervals were not calculated.
The workloads are synthetic and omit CSS resolution, text shaping, replaced
content, positioned descendants, and application mutation patterns. The
results should guide profiling priorities, not justify an engine choice by
themselves.
