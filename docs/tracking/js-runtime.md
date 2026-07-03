# JS/Lepus runtime bindings, native modules & lifecycle

> Status: pending initial research pass — see [README.md](README.md).

Will cover: the global `lynx` object API (`lynx.getJSModule`,
`lynx.reportError`, `lynx.performance`, `lynx.requestAnimationFrame`,
`lynx.getElementById`/`createSelectorQuery`, ...), the native module
invocation mechanism, `TemplateData`/`GlobalProps` injection, app/page
lifecycle hooks (`onAppReload`, page show/hide, `onLoad`, ...), the Element
PAPI (`__CreateElement`/`__SetAttribute`/`__AppendElement`/`__FlushElementTree`-style
low-level calls used by compiled output), and which parts of this surface are
main-thread-only vs background-thread-only vs shared.

Scope note: this is the spec for `.claude/agents/lynx-js-runtime-bridge.md`.
