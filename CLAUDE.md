# lynx-vello

Canonical project context — mission, standards policy, dependency policy,
crates, reference repos, toolchain, testing — lives in
[AGENTS.md](AGENTS.md). **Read that first.** This file only adds
Claude-Code-specific notes on top of it.

## Claude-Code-specific

- Skills: [`.claude/skills/lynx-template-format`](.claude/skills/lynx-template-format/SKILL.md)
  — byte-level `.web.bundle`/`.lynx.bundle` format knowledge, triggers
  automatically for template/format work.
- Subagents: [`.claude/agents/`](.claude/agents/) — specialized personas for
  the style/layout/text/render/runtime/ReactLynx-compat subsystems described
  in `AGENTS.md` and `docs/tracking/`. Prefer delegating subsystem work to the
  matching subagent over researching that subsystem from scratch in the main
  thread.
- Format with `cargo fmt` before finishing any Rust change (nightly rustfmt
  options in `rustfmt.toml`).
