# Task-kickoff prompts

Copy-pasteable prompt templates for recurring lynx-vello work. Usable from
either Claude Code (paste directly, or as the prompt to the matching subagent
in `.claude/agents/`) or Codex (paste into the `codex` CLI, which reads
`AGENTS.md` on its own). Fill in the `{PLACEHOLDERS}`.

All of these assume the reader starts from `AGENTS.md` — they don't repeat the
project mission/standards policy, just the task-specific framing.

Three reference repos, three different roles (absolute paths defined once in
`AGENTS.md`'s "Reference repos"; shorthand used below):

- `lynx/` — Lynx behavior spec (C++ engine)
- `lynx-stack/` — Lynx/ReactLynx behavior spec (TS/Rust, web target);
  `packages/web-platform/web-core` is the compatibility target
- `Paws/` — **not** Lynx behavior; an implementation-pattern reference for
  DOM/CSS system design (stylo wiring, stacking context, event
  dispatch/hit-testing) only

## Extend or verify a tracking doc

Use when a `docs/tracking/*.md` file doesn't cover the edge you need, or when a
line citation in one needs re-checking. Every file there is written — none is a
stub — so the job is the delta, not a first pass.

```
Read AGENTS.md and docs/tracking/README.md for context, then read
{TRACKING_FILE} for what is already documented about {TOPIC}. Research only
what is missing or stale by reading the actual source in lynx/ and/or
lynx-stack/ (see AGENTS.md "Reference repos" for their absolute paths; don't
rely on general knowledge of LynxJS — confirm by reading real files, and cite
the paths and line numbers you read). Re-verify any existing citation you rely
on; those line numbers were checked once, when the row was written. Follow the
column conventions in docs/tracking/README.md. Classify each finding into the
two buckets in AGENTS.md's standards policy, and surface any native-Lynx vs
web-core conflict to me rather than resolving it. Update {TRACKING_FILE} in
place.
```

## Implement a CSS property

The order is fork grammar first, then the engine, then the UA sheet, then
tests — a property that isn't in the fork's `lynx` allowlist doesn't parse.

```
Read AGENTS.md (standards policy, the crates/dom entry and its vendored-stylo
paragraphs), docs/style-architecture.md, docs/style-assumptions.md, and
docs/tracking/{css-layout,css-visual,css-text,css-animation,
css-selectors-cascade}.md for {PROPERTY_NAME}. If the tracking doc doesn't
cover this property's edge, research it directly against lynx/ and lynx-stack/
and cite what you read.

1. Grammar: check vendor/stylo/style/properties/lynx_properties.txt and the
   longhand/shorthand tables. If the property or a value of it is missing, that
   is a vendor/stylo fork patch first, with a lynx_* test under
   vendor/stylo/style/tests/.
2. Engine: crates/dom/src/style/ for cascade/computed/invalidation, and
   crates/bobcat-source/src/lower_style.rs +
   crates/bobcat-core/src/style.rs if the .web.bundle StyleInfo path carries it.
3. UA policy: crates/bobcat-core/src/main/tree/ua_sheet.rs and the per-tag
   rules beside it, if Lynx gives the property a non-CSS default.
4. Tests: a grammar/cascade test under crates/dom/tests/, plus a screenshot
   case if it paints.

Implement the W3C-correct behavior unless the property is a Lynx-only
extension with no spec equivalent, in which case match Lynx exactly and do not
extend it. Record any confirmed divergence in docs/tracking/deviations.md.
```

## Port a built-in component

```
Read AGENTS.md and docs/tracking/components.md for {COMPONENT} (e.g. x-list,
x-swiper). Read the reference implementation at
lynx-stack/packages/web-platform/web-elements for its current DOM/CSS-based
behavior, and lynx/ for the native element it mirrors. Implement the
equivalent natively: the Lynx element policy layer is
crates/bobcat-core/src/main/tree/ (see text.rs, raw_text.rs, image.rs and
scroll_container.rs for the existing shapes), registered through
dom::CustomElement, with its UA box authored in the UA sheet. Delegate layout
to hughie, paint to dom's visual/paint modules and text to
hughie::text::block rather than reimplementing those concerns inline. Add a
crates/bobcat-core test and a screenshot case. Note any behavior you couldn't
verify against real source.
```

## Audit a JS/runtime API for parity

```
Read AGENTS.md ("JavaScript data ownership" and the crates/bobcat-core entry),
then the runtime doc that owns {API_NAME}: docs/mts-execution-runtime.md,
docs/data-lifecycle-runtime.md, docs/destruction-runtime.md,
docs/events-diagnostics-runtime.md, docs/node-query-runtime.md,
docs/worker-resources-runtime.md or docs/named-styles-runtime.md, plus
docs/tracking/{js-runtime,web-core-runtime,dom-events}.md. Compare against
what this repo implements: packages/bobcat-element/src/ (element-papi.ts's
header table is the authoritative PAPI list, native.d.ts the host-member
contract, main-thread-runtime.ts / background-thread-runtime.ts /
worker.ts / cross-thread-context.ts the realm surfaces) and
crates/bobcat-core/src/main/runtime/. Confirm the exact signature, which
thread it runs on, and its side effects by reading
lynx-stack/packages/web-platform/web-core and lynx/core/runtime. Report what
matches and what is missing — do not implement fixes unless asked.
```

## Investigate a ReactLynx compatibility gap

```
Read AGENTS.md and docs/tracking/reactlynx.md. A ReactLynx app does {SYMPTOM}
under lynx-vello but does {EXPECTED} under real web-core. Reproduce it against
a compiled card: build packages/reactlynx-test-fixtures
(pnpm --filter reactlynx-test-fixtures build) or render the bundle through
bobcat-server (crates/bobcat-cli, server feature — see
crates/bobcat-cli/SERVER.md). Read lynx-stack/packages/react/runtime for the
expected reconciliation behavior first (thread, ordering, snapshot/patch
semantics), then decide whether the gap is in the runtime bridge underneath
(wrong thread, wrong event kind, a structured-clone refusal, a timing
contract) or in the compat layer itself before proposing a fix.
```

## Replicate a lynx-stack test natively

`docs/tracking/web-text-test-replication.md` is the worked example: every text
test in `lynx-stack/packages/web-platform` catalogued, ported, and its gaps and
native-vs-web-core conflicts recorded with the user's rulings.

```
Read AGENTS.md and docs/tracking/web-text-test-replication.md as the worked
example of this task's shape and its status-tracker conventions. Catalogue
every test under lynx-stack/{TEST_PATH} that covers {AREA}. For each one,
write the native equivalent in this repo ({TEST_CRATE}/tests/), asserting
web-core's behavior — that is the compatibility target. Where a case cannot
pass today, land it as a recorded GAP row rather than deleting it, and say
what engine work would close it. Where native Lynx and web-core disagree,
report both and ask me which to assert; do not decide silently. Screenshot
cases go through flashbulb: look at the PNG before accepting a golden with
FLASHBULB_UPDATE_SNAPSHOTS=1, and remember a newly created golden fails its
own run by design.
```

## Hand a hard implementation problem to Codex

Codex reads `AGENTS.md` automatically, so the hand-off only needs the task and
its pointers:

```
Context: lynx-vello, a Rust reimplementation of LynxJS's web-bundle runtime on
stylo/vello/parley (see AGENTS.md for the mission and the standards policy).
Task: {TASK}. Relevant spec: docs/tracking/{FILE}.md and docs/{DOC}.md.
Relevant code: {CRATE_PATH}. Relevant reference source: lynx/{PATH} or
lynx-stack/{PATH} (see AGENTS.md "Reference repos" for their absolute paths;
use Paws/{PATH} instead for a DOM/CSS implementation-pattern question, not for
Lynx behavior). Before finishing: ./.github/scripts/fmt-check.sh (not
cargo fmt --all), pnpm install --frozen-lockfile and
pnpm --filter reactlynx-test-fixtures build, then
cargo clippy --all-targets -- -D warnings and the subsystem's tests. The PR
body needs before/after Mermaid diagrams.
```
