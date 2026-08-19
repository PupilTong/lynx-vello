# lynx-vello — Codex Override

Codex selects this file instead of the root `AGENTS.md`. Before doing any work,
read `AGENTS.md` completely and follow it as the canonical project and
architecture guide. This file only adds Codex-specific operating constraints;
it does not replace or weaken any instruction in `AGENTS.md`.

## Working with Codex

Division of labor between Claude Code and Codex is **not yet decided** beyond
Codex's existing rescue / second-opinion / review role (`codex:codex-rescue`,
`/codex:review`). Do not assume Codex owns any particular crate or subsystem
unless a task explicitly says so.

### GPU commands under the Codex sandbox

On macOS, Codex's `workspace-write` Seatbelt sandbox cannot enumerate Metal
adapters even when the host has a usable GPU. Any command that initializes
Metal, wgpu, or Vello — including GPU-backed tests, screenshot tests, headed or
headless runners, and render benchmarks — must therefore be run outside the
sandbox with an explicit sandbox escalation. Request that escalation before
the first attempt and keep reusable approvals scoped to the exact GPU command
prefix. A `GpuError::NoAdapter` or equivalent adapter failure from inside the
sandbox is an environment failure, not a product failure; rerun the same
command with escalation before diagnosing the renderer. Compilation,
formatting, linting, and CPU-only tests remain in the normal sandbox unless they
independently require broader access.

### Git and pull requests under the Codex sandbox

Codex worktrees keep their Git metadata outside the writable worktree through
the `.git` indirection. Commands that mutate that metadata — including branch
creation, staging, and committing — and networked Git commands such as `push`
must therefore request an explicit sandbox escalation before the first attempt.
Keep reusable approvals scoped to the exact Git subcommand rather than granting
general shell access, and do not diagnose a sandbox denial as repository
corruption or an authentication failure.

After the branch is pushed with local `git`, create pull requests only through
the installed GitHub plugin/app connector. Never use the GitHub CLI (`gh`) to
create, submit, publish, or otherwise open a pull request. If the connector is
unavailable or cannot create the pull request, stop and ask the user for
direction instead of falling back to `gh`; local `gh` authentication is not a
substitute for connector authorization.
