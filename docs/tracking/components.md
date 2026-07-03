# Built-in components (web-elements ↔ Lynx JSX tags)

> Status: pending initial research pass — see [README.md](README.md).

Will enumerate every custom element implemented in `lynx-stack`'s
`packages/web-platform/web-elements` (`x-view`, `x-text`, `x-image`, `x-list`,
`x-scroll-view`, `x-swiper`, `x-input`, `x-textarea`, `x-overlay`/`x-modal`,
`x-foldview-*`, etc.), the corresponding Lynx JSX tag it backs (`<view>`,
`<text>`, `<image>`, `<list>`, `<scroll-view>`, ...), its purpose, and any
attribute/CSS default behaviors or quirks (list virtualization, swiper snap
behavior, native-feeling scroll physics, ...) a from-scratch implementation
must replicate.

Scope note: this is a spec for `.claude/agents/lynx-reactlynx-compat.md`
(component-level compatibility) in cooperation with the render/layout/text
agents for how each component actually paints.
