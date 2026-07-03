# ReactLynx runtime, compiler model & public API

> Status: pending initial research pass — see [README.md](README.md).

Will cover: the JSX runtime and snapshot/patch reconciliation model
(background-thread reconciliation vs main-thread rendering split, i.e. the
"ReactLynx 3.0" architecture) from `lynx-stack`'s `packages/react/runtime` and
`packages/react/transform`; hooks support (`useState`/`useEffect`/`useMemo`/`useCallback`/`useRef`/`useContext`
plus Lynx-specific hooks); refs to native elements; the `main-thread:`
event-handler directive and main-thread functions (MTS); list
rendering/reuse-identifier diffing; error boundaries; context providers;
background-only vs main-thread-only API restrictions; and the public API
surface from `packages/react/components` (`<list>`, `<scroll-view>`,
`<text>`/`<raw-text>`, event prop naming like `bindtap`/`catchtap` vs JSX
`onTap`-style props, exported utility hooks).

Scope note: this is the spec for `.claude/agents/lynx-reactlynx-compat.md`.
