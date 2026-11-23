# lynx-vello

@AGENTS.md

The import above is the canonical project context: mission, standards policy,
where new code goes, the workspace map, reference repos, toolchain, testing.
Before working in a crate or package, also read that crate's own `AGENTS.md`,
linked from the Workspace map. The rest of this file is Claude-Code-specific.

## Subagents ([`.claude/agents/`](.claude/agents/))

Implementation inside a subsystem goes to the matching persona:

- `lynx-css-engine` — StyleInfo decode/lowering, cascade, computed style,
  selectors, at-rules, UA sheet, the stylo fork's `lynx` grammar.
- `lynx-layout-engine` — `seldon` and its `terminus` host: Flexbox, Grid, Linear,
  Relative, containment, the positioned pass.
- `lynx-text-engine` — the `<text>` block on parley: shaping, truncation, fonts.
- `lynx-render-engine` — stacking, paint order, the vello scene, committed
  frames and compose program, scroll composition, animations.
- `lynx-js-runtime-bridge` — QuickJS realms, MTS/BTS threads, Workers, the
  Element PAPI in `packages/bobcat-element`, timers, ESM, events.
- `lynx-reactlynx-compat` — compiled ReactLynx apps end to end: fixtures, page
  data, selector queries, censuses over `bobcat-server`.
- `lynx-behavior-researcher` — read-only spec research against `lynx/`,
  `lynx-stack/`, `Paws/`. Only the main session can invoke it.

A persona names the paths an agent works in by default. Which crate new code
goes into is decided by AGENTS.md "Where new code goes", for every agent.

## What the main session keeps when it delegates

A subagent knows its brief and reports what it added. The main session is the
one that can see whether the result fits the rest of the tree.

- **Read the code before a claim about it.** Before saying how something works
  or what a change needs, read the code path and give `file:line`. A
  conversation summary, a memory entry and a subagent's report say where to
  look; none of them is evidence.
- **A brief states**, for each piece of the task: the crate that owns it; the
  existing functions to reuse, with their paths; what must not change; the
  checks to run. It asks the report to list each new file with the reason it
  is in that crate, anything written that repeats existing code, and the checks
  not run.
- **A review reads the diff, not the report.** For each new file, and each new
  parser, value conversion or keyword table: is this the owning crate, and does
  the tree or `vendor/stylo` already have it? Passing tests answer neither
  question.
- **A move or a refactor follows the rule, not the old directory.** When code
  of one kind moves to its owning crate, look for the rest of that kind outside
  the directory being moved.

## Skill

[`.claude/skills/lynx-template-format`](.claude/skills/lynx-template-format/SKILL.md)
— byte-level `.web.bundle`/`.lynx.bundle`/XML knowledge over
`crates/bobcat-source`; triggers automatically for format work.

## Mirrors

`.codex/agents/*.toml` and `.agents/skills/` mirror `.claude/agents/` and
`.claude/skills/` for Codex. Update them in the same change.
