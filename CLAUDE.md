# lynx-vello

Canonical project context — mission, standards policy, dependency policy,
crates, reference repos, toolchain, testing — lives in
[AGENTS.md](AGENTS.md). **Read that first.** This file only adds
Claude-Code-specific notes on top of it.

## Subagents ([`.claude/agents/`](.claude/agents/))

Delegate subsystem work to the matching persona rather than researching that
subsystem from scratch in the main thread.

- `lynx-css-engine` — StyleInfo decode/lowering, cascade, computed style,
  selectors, at-rules, UA sheet, the stylo fork's `lynx` grammar.
- `lynx-layout-engine` — `hughie` and its `dom` host: Flexbox, Grid, Linear,
  Relative, containment, the positioned pass.
- `lynx-text-engine` — the `<text>` block on parley: shaping, truncation, fonts.
- `lynx-render-engine` — stacking, paint order, the vello scene, committed
  frames and compose program, scroll planes, animations.
- `lynx-js-runtime-bridge` — QuickJS realms, MTS/BTS threads, Workers, the
  Element PAPI in `packages/bobcat-element`, timers, ESM, events.
- `lynx-reactlynx-compat` — compiled ReactLynx apps end to end: fixtures, page
  data, selector queries, censuses over `bobcat-server`.
- `lynx-behavior-researcher` — read-only spec research against `lynx/`,
  `lynx-stack/`, `Paws/`. Only the main session can invoke it.

## Skill

[`.claude/skills/lynx-template-format`](.claude/skills/lynx-template-format/SKILL.md)
— byte-level `.web.bundle`/`.lynx.bundle`/XML knowledge over
`crates/bobcat-source`; triggers automatically for format work.

## Commands

- Format with `cargo fmt -p <crate>` per crate touched, never `cargo fmt --all`
  (it reaches `vendor/stylo`); then run CI's `./.github/scripts/fmt-check.sh`.
- `pnpm install --frozen-lockfile` and
  `pnpm --filter reactlynx-test-fixtures build` before any cargo command.
- `pnpm test:type` (and `pnpm --filter bobcat-element test`) when TS changed.

## Mirrors

`.codex/agents/*.toml` and `.agents/skills/` mirror `.claude/agents/` and
`.claude/skills/` for Codex. Update them in the same change.
