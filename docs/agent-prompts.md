# Task-kickoff prompts

Copy-pasteable prompt templates for recurring lynx-vello work. Usable from
either Claude Code (paste directly, or as the prompt to the matching subagent
in `.claude/agents/`) or Codex (paste into `codex` CLI, or forward through
`/codex:rescue`). Fill in the `{PLACEHOLDERS}`.

All of these assume the reader starts from `AGENTS.md` — they don't repeat the
project mission/standards policy or its owner-thread/non-reentrant-flush
constraints, just the task-specific framing. Do not add synchronization for a
hypothetical cross-thread VM/Widget/Document model when using these prompts.

Three reference repos, three different roles (absolute paths defined once in
`AGENTS.md`; shorthand used below):

- `lynx/` — Lynx behavior spec (C++ engine)
- `lynx-stack/` — Lynx/ReactLynx behavior spec (TS/Rust, web target)
- `Paws/` — **not** Lynx behavior; an implementation-pattern reference for
  DOM/CSS system design (stylo wiring, stacking context, event
  dispatch/hit-testing) only

## Research a tracking-doc gap

Use when a `docs/tracking/*.md` file is still a stub, or you need to verify a
specific behavior isn't already covered before extending it.

```
Read AGENTS.md and docs/tracking/README.md for context. Research {TOPIC} by
reading the actual source in lynx/ and/or lynx-stack/ (see AGENTS.md for
their absolute paths; don't rely on general knowledge of LynxJS — confirm by
reading real files, and cite the paths you read). Follow the
column conventions in docs/tracking/README.md. Flag anything that deviates
from the relevant W3C/CSS/DOM standard, per the standards policy in
AGENTS.md. Update {TRACKING_FILE} with your findings — replace the "pending
initial research pass" stub content, don't just append.
```

## Implement a CSS property

```
Read AGENTS.md, then docs/tracking/{css-layout,css-visual,css-text,css-animation}.md
for {PROPERTY_NAME}'s spec — if that file is still a stub for this property,
research it directly against lynx/ and lynx-stack/ yourself, or (only if
you're the main session, since subagents can't spawn other subagents) run
the lynx-behavior-researcher subagent first. Implement
{PROPERTY_NAME} in the style engine (stylo integration), matching Lynx's
behavior unless docs/tracking/deviations.md (or your own research) says it
diverges from the W3C spec, in which case implement the W3C-correct behavior.
Add a test that exercises it end-to-end from a decoded StyleInfo fixture if
one covers it, or a targeted unit test otherwise.
```

## Port a built-in component

```
Read AGENTS.md and docs/tracking/components.md for {COMPONENT} (e.g. x-list,
x-swiper). Read the reference implementation at
lynx-stack/packages/web-platform/web-elements for its current DOM/CSS-based
behavior. Implement the equivalent behavior natively for lynx-vello,
delegating layout to lynx-layout-engine, paint to lynx-render-engine, and
text to lynx-text-engine as needed rather than reimplementing those concerns
inline. Note any behavior you couldn't verify against real source.
```

## Audit a JS/runtime API for parity

```
Read AGENTS.md and docs/tracking/js-runtime.md (and web-core-runtime.md /
dom-events.md if relevant) for {API_NAME}. Confirm its exact signature,
timing (main-thread vs background-thread vs shared), and side effects by
reading lynx/core/runtime and lynx-stack/packages/web-platform/web-core.
Report whether lynx-vello's current implementation (if any) matches, and
what's missing — do not implement fixes unless asked.
```

## Investigate a ReactLynx compatibility gap

```
Read AGENTS.md and docs/tracking/reactlynx.md. A ReactLynx app does
{SYMPTOM} under lynx-vello but does {EXPECTED} under real web-core. Read
lynx-stack/packages/react/runtime to understand the expected reconciliation
behavior first (thread, ordering, snapshot/patch semantics), then check
whether the gap is actually in lynx-js-runtime-bridge (wrong runtime timing
contract) or in the ReactLynx-compat layer itself before proposing a fix.
```

## Hand a hard implementation problem to Codex

Use `/codex:rescue` (or the `codex:codex-rescue` subagent) rather than typing
this by hand — but if composing the forwarded task text yourself, include:

```
Context: lynx-vello, a Rust reimplementation of LynxJS's web-bundle runtime
on stylo/vello/parley (see AGENTS.md for the full mission and standards
policy — Codex reads AGENTS.md automatically). Task: {TASK}. Relevant spec:
docs/tracking/{FILE}.md. Relevant reference source: lynx/{PATH} or
lynx-stack/{PATH} (see AGENTS.md for their absolute paths; use Paws/{PATH}
instead for a DOM/CSS implementation-pattern question, not Lynx behavior).
```
