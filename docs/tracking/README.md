# lynx-vello behavior/feature tracking

This directory is the running inventory of everything the [`lynx`](https://github.com/lynx-family/lynx)
engine and [`lynx-stack`](https://github.com/lynx-family/lynx-stack) (ReactLynx +
`web-core`) do that lynx-vello needs to match, scoped to the **web-bundle**
runtime (see [`AGENTS.md`](../../AGENTS.md) for the project mission). It exists
so implementation work on any subsystem starts from a real spec instead of
re-deriving Lynx behavior from scratch each time — **read the relevant file
below before implementing a new subsystem.**

A third local checkout, `Paws/` (absolute path defined once in `AGENTS.md`),
is *not* a Lynx behavior spec — it's a sibling native Rust UI engine (stylo +
Taffy + parley) used purely as an **implementation-pattern** reference for
DOM system and CSS system design (wiring stylo onto a custom DOM, stacking
context, event dispatch/hit-testing). It's cited in the files below where
relevant; see `AGENTS.md` for its full scope.

Nothing in this repo implements any of this yet; every file here is pure
research/spec, not a status tracker of finished work. Once implementation
starts, each item should gain an explicit done/in-progress marker in its row.

## Column conventions

Most files use a table with these columns:

| Column | Meaning |
| --- | --- |
| Item | Property/API/behavior name |
| Description | What it does, in Lynx today |
| Tier | `Core` (must-have for basic ReactLynx apps to render/work), `Extended` (common but not universal), `Rare` (long-tail, defer) |
| W3C-compliant? | `Yes` / `No` / `Partial` — whether Lynx's behavior matches the relevant web standard |
| Deviation & what to do instead | Only filled when not compliant — the actual W3C-correct behavior to implement instead (see the [standards policy](../../AGENTS.md#standards-policy-w3c-first-lynx-behavior-second)) |
| Source refs | File paths into the local `lynx`/`lynx-stack`/`Paws` checkouts, prefixed `lynx/`, `lynx-stack/`, or `Paws/` (the last only on implementation-pattern rows, never as a behavior-spec source) |

## Index

| File | Domain |
| --- | --- |
| [css-layout.md](css-layout.md) | Box model, positioning, flex, Lynx's `linear`/`relative` layout, z-index/stacking |
| [css-visual.md](css-visual.md) | Color, background, border, shadow, filter, transform, opacity |
| [css-text.md](css-text.md) | Font/text properties relevant to `parley` shaping/layout |
| [css-animation.md](css-animation.md) | Transitions, `@keyframes`, timing functions, JS `animate()` API |
| [dom-events.md](dom-events.md) | Element/NodesRef model, event types, bind/catch/capture-bind/capture-catch model, gesture recognizers |
| [js-runtime.md](js-runtime.md) | Global `lynx` object, native modules, Element PAPI, app/page lifecycle, main/background threading |
| [web-core-runtime.md](web-core-runtime.md) | `web-core`'s dual-thread architecture end to end — the reference model lynx-vello replicates natively |
| [components.md](components.md) | Built-in components (`web-elements` custom elements ↔ Lynx JSX tags) |
| [reactlynx.md](reactlynx.md) | ReactLynx runtime/compiler model, hooks compatibility matrix, public API & component library |
| [deviations.md](deviations.md) | Rollup of every known Lynx-vs-W3C behavior divergence pulled from the files above, with the W3C-correct behavior to implement |

## Status

Initial research pass in progress (multi-agent sweep over both reference
repos). Files will be filled in as that completes.
