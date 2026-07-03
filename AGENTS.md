# lynx-vello — Agent Guide

This is the canonical project/architecture doc for coding agents working in this
repo (Claude Code and Codex both start here — `CLAUDE.md` is a short pointer to
this file plus Claude-specific notes).

## Mission

lynx-vello is a from-scratch Rust reimplementation of the LynxJS **web-bundle**
runtime — the same runtime [`lynx-stack`](https://github.com/lynx-family/lynx-stack)'s
`web-core` package implements today inside a browser (a dual-thread JS runtime +
DOM + CSS engine). We replace that browser-hosted implementation with a native,
cross-platform engine built on:

- **[stylo](https://github.com/servo/stylo)** — CSS parsing/cascade/computed-style engine (Servo's)
- **[vello](https://github.com/linebender/vello)** — GPU vector rendering
- **[parley](https://github.com/linebender/parley)** — text layout & shaping

A from-scratch layout engine (successor to the C++ engine's `starlight`) is
planned but not yet started; see `docs/tracking/css-layout.md` for the behavior
it will need to cover.

**Compatibility target**: ReactLynx apps compiled to `.web.bundle` must render
and behave the same as they do under `web-core` today. "Behave the same" means
matching rendering output and user-interaction behavior — **not** pixel-perfect
fidelity, and **not** reimplementing Android/iOS native platform code paths.
This project does not touch the native `.lynx.bundle` format or platform
bridges (`docs/lynx-binary-template.md` is kept for reference only, not a
target).

## Standards policy: W3C first, Lynx behavior second

Match Lynx's *observable* behavior only where it doesn't conflict with
W3C/CSS/DOM standards. Where the actual LynxJS engine's behavior diverges from
the relevant web standard, implement the **W3C-standard behavior instead** of
replicating the Lynx quirk. Known example: Lynx's `z-index`/stacking
implementation does not follow the CSS stacking-context algorithm — implement
the real CSS algorithm instead. See `docs/tracking/deviations.md` for the
running list of known divergences found so far.

## Dependency policy

All crates should track the **latest available versions** — **except `rkyv`,
pinned to `0.7`** (see `[workspace.dependencies]` in the root `Cargo.toml`)
because the `.web.bundle` `StyleInfo` section is a previously-serialized rkyv
0.7 wire format produced by existing `web-core` bundles; we must stay able to
decode those without a forward-compat break. `/Users/akiwah/repos/paws-libs/Paws`'s
`Cargo.toml` (an actively maintained sibling project on `stylo`/`parley`) is a
useful signal for currently-compatible versions of those libraries.

## Crates

- `crates/lynx-template-decoder` — decodes `.web.bundle` (magic `SDRA WROF`):
  manifest, rkyv `StyleInfo`, Lepus/JS code, custom sections. Scope: binary
  template parsing only, no JS runtime, no CSS engine (yet).
- *(planned, not yet scaffolded)* style / layout / text / render / runtime
  crates — see `docs/tracking/` for the behavior surface each will need to
  cover before scaffolding begins, and `.claude/agents/` for the
  subsystem-scoped agent personas already set up for this work.

## Reference repos (local checkouts, read-only — do not edit)

- `/Users/akiwah/repos/lynx` — the original LynxJS engine (C++). Ground truth
  for CSS/DOM/event/animation *semantics*. We do not reimplement its
  Android/iOS/native-bundle platform code.
- `/Users/akiwah/repos/lynx-stack` — TS/Rust monorepo: `packages/react/*`
  (ReactLynx framework) and `packages/web-platform/*` (`web-core` dual-thread
  runtime, `web-elements` built-in components). This is the architectural
  reference for the dual-thread execution model lynx-vello must replicate
  natively (no literal worker/iframe threads).
- `/Users/akiwah/repos/paws-libs/Paws` — a sibling native Rust UI engine
  (`stylo` + Taffy + `parley`, WASM-driven, UIKit/wgpu-painted). **Not** a
  Lynx project and **not** a behavior spec — it's an implementation-pattern
  reference for DOM system and CSS system design: how to wire `stylo`'s
  cascade/`RuleTree` onto a custom arena-based DOM (`engine/src/dom/`,
  `engine/src/style.rs`, `engine/src/style/css_style_sheet.rs`), a real
  spec-conformant CSS stacking-context implementation
  (`engine/src/layout/stacking.rs` — relevant to the z-index deviation
  above), and DOM-style event dispatch/hit-testing with no browser
  underneath (`engine/src/events/`, `engine/src/hit_test/`). Its
  `paws-style-ir/` crate is a second, independent rkyv-based style-IR design
  worth comparing against our own `RawStyleInfo` (it targets rkyv `0.8.9`;
  ours stays pinned at `0.7`, see Dependency policy below).

## Reference knowledge

- `docs/web-binary-template.md` — the web-target bundle format this repo
  decodes today: container layout, section encodings, and the rkyv 0.7
  `RawStyleInfo` CSS data model (mirrored 1:1 in the decoder crate — field/variant
  order there is wire format, do not reorder).
- `docs/lynx-binary-template.md` — the *native* `.lynx.bundle` format ("lynx"
  target), reference only, not implemented here.
- `docs/tracking/` — the behavior/feature inventory (CSS properties, layout
  algorithms, DOM/event model, JS runtime APIs, `web-core` runtime
  architecture, built-in components, ReactLynx surface) that future
  implementation work is scoped against. **Read the relevant file before
  implementing any new subsystem.** Start at `docs/tracking/README.md`.
- `docs/agent-prompts.md` — copy-pasteable task-kickoff prompts for recurring
  work (adding a CSS property, porting a built-in component, auditing a JS API
  for parity, etc.), usable from either Claude Code or Codex.

## Toolchain

- Nightly Rust (`rust-toolchain.toml`), edition 2024, resolver 3, workspace lints.
- `cargo fmt` (nightly rustfmt options in `rustfmt.toml`), `cargo clippy`,
  `cargo test`, `cargo bench` (CodSpeed-compatible `divan` benches).

## Testing

Integration tests decode real fixtures vendored from lynx-stack under
`crates/lynx-template-decoder/tests/fixtures/` (Apache-2.0 build artifacts).
`cargo test` must pass on the pinned nightly toolchain.

## Working with Codex

This repo is worked on by both Claude Code (reads `CLAUDE.md`, which points
here) and Codex (reads this file directly). Division of labor between them is
**not yet decided** beyond Codex's existing rescue / second-opinion / review
role (`codex:codex-rescue`, `/codex:review`) — don't assume Codex owns any
particular crate or subsystem unless a task explicitly says so.
