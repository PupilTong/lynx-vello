# CSS box model, positioning & layout

> Status: pending initial research pass — see [README.md](README.md).

Will cover: `display` modes, box model (width/height/margin/padding/border/box-sizing),
sizing units (`px`/`rpx`/`ppx`/`%`/`vw`/`vh`/`env()`), flex layout, Lynx's
non-standard `linear-*` layout (`display: linear`, `linear-weight`,
`linear-gravity`, `linear-direction`) and `relative-*` layout (not the same as
CSS `position: relative`), grid support if any, `position` modes, `overflow`,
`visibility`, and `z-index`/stacking context (known W3C deviation — see
[deviations.md](deviations.md)).

Scope note: this is the behavior spec for the *layout algorithm*, which the
planned from-scratch layout engine (successor to the C++ engine's `starlight`)
will implement — see `.claude/agents/lynx-layout-engine.md`.

Implementation-pattern reference (not a behavior spec):
`/Users/akiwah/repos/paws-libs/Paws`'s `engine/src/layout/stacking.rs` for a
real, WPT-conformance-tested CSS stacking-context implementation over
`stylo` computed style — the concrete reference for the z-index deviation.
