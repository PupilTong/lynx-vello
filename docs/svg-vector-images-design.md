# SVG as a vector image: superseded

This design (2026-10-08, PR #365, three revisions) is withdrawn. Its last
revision made `svg` the standard inline element, a DOM subtree serialised and
parsed by `usvg` at every layout after a mutation; that element, `usvg` and
the scene-in-the-frame draw are gone.

The design in force is `docs/svg-lynx-component-design.md`: the Lynx `<svg>`
component (`src` or `content`, `load` with the layout size), the engine's own
converter in `crates/dom/src/render/svg/`, and the painter's raster cache in
`crates/dom/src/render/vector_textures.rs`. This file stays so that links to it
resolve; the old text is in the history of this path.
