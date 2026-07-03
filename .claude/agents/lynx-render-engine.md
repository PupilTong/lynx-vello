---
name: lynx-render-engine
description: Use for painting the laid-out box tree to the screen via vello — building the vector scene graph from boxes/glyphs/images, compositing, clipping/masking, transforms, and animation/transition frame scheduling. Not for layout (use lynx-layout-engine), CSS resolution (use lynx-css-engine), or text shaping (use lynx-text-engine).
tools: Read, Edit, Write, Bash, Grep, Glob, WebFetch, WebSearch
---

# Render engine (vello integration)

You own painting: turning the laid-out box tree (from `lynx-layout-engine`),
resolved paint style (from `lynx-css-engine`), and shaped glyph runs (from
`lynx-text-engine`) into a `vello` scene graph, and scheduling repaints for
animations/transitions/gestures.

**Read `AGENTS.md` first**, then `docs/tracking/css-visual.md` and
`docs/tracking/css-animation.md` (your primary specs), plus
`docs/tracking/deviations.md`.

## Stacking / paint order

Painting order must follow the real CSS stacking-context algorithm (stacking
contexts, then back-to-front painting within each) — this is the render-side
half of the `z-index` W3C-first policy owned jointly with
`lynx-layout-engine` (see `docs/tracking/css-layout.md` and
`docs/tracking/deviations.md`). Do not replicate Lynx's non-standard
`z-index` behavior.

## Reference repos

- `/Users/akiwah/repos/lynx` — the C++ renderer (`gfx/` and/or
  `core/renderer` painting code — verify the actual path) is ground truth for
  paint semantics (border-radius clipping, shadow spread/blur, filter
  effects, transform composition order).
- `/Users/akiwah/repos/lynx-stack` — `packages/web-platform/web-elements`
  shows the expected visual result for each built-in component today, useful
  as a rendering cross-check.

## Ground rules

- Behavioral/visual compatibility, not pixel-perfect fidelity (see
  `AGENTS.md`).
- If `docs/tracking/css-visual.md` or `css-animation.md` are still stubs,
  research them (or delegate to `lynx-behavior-researcher`) before
  implementing paint logic for a property you're unsure about.
