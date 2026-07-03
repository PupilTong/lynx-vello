# CSS animation, transition & JS animate() API

> Status: pending initial research pass — see [README.md](README.md).

Will cover: `transition-*`, `animation-*`, `@keyframes` support, timing
functions (including any Lynx-specific curves, e.g. spring physics), and the
`element.animate()`-style JS animation API exposed to Lepus/JS (method
surface, options, fired events).

Scope note: this feeds both the style engine (parsing/interpolation) and the
render engine (frame scheduling/compositing) — see
`.claude/agents/lynx-css-engine.md` and `.claude/agents/lynx-render-engine.md`.
