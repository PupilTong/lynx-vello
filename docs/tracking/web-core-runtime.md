# web-core dual-thread runtime architecture

> Status: pending initial research pass — see [README.md](README.md).

Will describe, end to end, how `lynx-stack`'s `packages/web-platform/web-core`
splits main-thread and background-thread execution, the message-passing
protocol between them, how decoded template sections (Manifest/StyleInfo/LepusCode
— already handled by `crates/lynx-template-decoder`) get applied to build the
live DOM/CSS, hydration behavior, and the shape of polyfilled globals
(`globalThis.lynx`, `SystemInfo`, etc.). This is the architecture lynx-vello
must replicate the *behavior* of natively, without literal worker/iframe
threads.

Scope note: this is the primary spec for `.claude/agents/lynx-js-runtime-bridge.md`
and informs `.claude/agents/lynx-reactlynx-compat.md`.
