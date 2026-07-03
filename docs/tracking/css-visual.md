# CSS visual & paint properties

> Status: pending initial research pass — see [README.md](README.md).

Will cover: `color`, `background-*` (color/image/gradient/position/size/repeat/origin/clip),
`border-*` (color/style/width/radius, per-corner), `box-shadow`, `filter`,
`backdrop-filter`, `clip-path`, `mask`, `outline`, `opacity`,
`transform`/`transform-origin`/`perspective`, plus any Lynx-specific value
syntax quirks vs standard CSS grammar.

Scope note: this is the spec for what the `stylo`-backed style engine must
resolve and what the `vello`-backed renderer must paint — see
`.claude/agents/lynx-css-engine.md` and `.claude/agents/lynx-render-engine.md`.

Implementation-pattern reference (not a behavior spec):
`Paws/engine/src/style.rs` and `Paws/engine/src/style/css_style_sheet.rs`
show a working `stylo` cascade integration over a custom Rust DOM.
