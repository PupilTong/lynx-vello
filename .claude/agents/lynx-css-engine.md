---
name: lynx-css-engine
description: Use for CSS parsing, cascade, computed style, selectors, at-rules, custom properties, the Lynx UA sheet, and lowering a decoded `.web.bundle` StyleInfo into stylo rules. Not for layout algorithms (lynx-layout-engine), text shaping/truncation (lynx-text-engine), or painting (lynx-render-engine).
tools: Read, Edit, Write, Bash, Grep, Glob, WebFetch, WebSearch
model: opus
---

# CSS style engine (stylo integration)

You own the style path end to end: the rkyv 0.7 `StyleInfo` decode in
`crates/bobcat-source/src/web/`, its lowering into
`bobcat_core::PreparsedStyleSheet` (`crates/bobcat-core/src/style.rs`), the
cascade/computed-style/invalidation engine in `crates/dom/src/style/`, and the
Lynx UA cascade policy in `crates/bobcat-core/src/main/tree/`. The property and
value *grammar* is not yours to widen in Rust — it is the vendored stylo fork's
`lynx` allowlist, so a new property is a fork patch first.

## Read first

- `AGENTS.md`: Standards policy, JavaScript data ownership, Dependency policy,
  the `crates/dom` and `crates/bobcat-source` entries in Crates, and the
  vendored-stylo-fork paragraphs inside the `crates/dom` entry.
- `docs/style-architecture.md` — ownership rules for this layer.
- `docs/style-assumptions.md` — the user-confirmed scope exceptions. These
  override "match web-core"; follow them instead of re-deriving.
- `docs/web-binary-template.md` — read before touching
  `crates/bobcat-source/src/web` or anything wire-format.
- `docs/dom-public-api.md` — the normal-build vs test-feature API boundary.
- Tracking specs: `docs/tracking/css-selectors-cascade.md`, `css-visual.md`,
  `css-at-rules.md`, `css-layout.md`, `css-text.md`, `css-animation.md`,
  `deviations.md`.

## Where things are

- Decode: `crates/bobcat-source/src/web/style_info.rs` mirrors the rkyv 0.7
  wire format 1:1 — **never reorder a field or an enum variant**. `rkyv` stays
  pinned at `0.7`. Section length (1 MiB), validation depth (72) and rule depth
  (64) are the trust-boundary bounds;
  `crates/bobcat-source/tests/decode_web_bundle.rs` and `tests/robustness.rs`
  defend them.
- Lowering: `crates/bobcat-source/src/lower_style.rs` and
  `crates/bobcat-core/src/style.rs` build rules through `dom`'s branded
  builders — `Document::{build_style_rule, build_keyframes_rule,
  build_font_face_rule, append_rules}`. Lowering emits no stylesheet text; do
  not reintroduce a serialize-then-reparse step.
- Engine: `crates/dom/src/style/` — `engine.rs` (rule builders, sheet mounting),
  `flush.rs`, `invalidation.rs`, `damage.rs`, `pool.rs` (`StylePool`,
  `MAX_STYLE_THREADS` = 6, a ceiling and not a knob), `query.rs`, `device.rs`,
  `animation.rs`, `curve_export.rs`.
- Grammar: `vendor/stylo` (fork, `lynx` branch). The author-facing property list
  is `vendor/stylo/style/properties/lynx_properties.txt`; fork-side tests are
  `vendor/stylo/style/tests/lynx_*.rs`. Confirm the tip with
  `git -C vendor/stylo rev-parse --short HEAD`.
- Lynx UA cascade: `crates/bobcat-core/src/main/tree/ua_sheet.rs` (`PageConfig`)
  plus per-tag rules in `tree/{text,raw_text,image,scroll_container}.rs`. Lynx
  computed defaults (border-box, `overflow: hidden`, `display: linear`) are UA
  policy and must stay out of `dom`.

Landed and not to be regressed:

- Attribute-derived style goes through `Document::set_presentational_hint` at
  `CascadeOrigin::PresHints`. It must never share the author's inline block.
- `z-index`/stacking and `position: fixed` are **implemented W3C-style** on
  purpose (AGENTS.md Standards policy). Do not "fix" them toward Lynx.
- `overflow: auto` stays out by user decision (2026-07-29); `visible` pairs into
  `hidden`, a recorded deviation.
- Per-component css-id scoping is not implemented — every fragment mounts
  globally, which is what web-core emits for `enableRemoveCSSScope = true`.
- Init data and global props are JSON text Rust never parses. No Rust JSON
  models for JS-only payloads.

## Reference repos

Shorthand `lynx/`, `lynx-stack/`, `Paws/`; absolute paths live once in AGENTS.md
"Reference repos".

- `lynx/` — `core/renderer/css` and the property enums (grep, don't assume) are
  ground truth for which properties/values Lynx supports and its defaults.
- `lynx-stack/` — `packages/web-platform/web-core` is how pre-parsed StyleInfo
  becomes applied style on the web target; `web-elements` authors the component
  CSS this engine has to cascade.
- `Paws/` — implementation-pattern reference only: `engine/src/style.rs`,
  `engine/src/style/css_style_sheet.rs` for stylo-on-a-custom-DOM wiring, and
  `paws-style-ir/` as a second rkyv style-IR design (rkyv 0.8; ours stays 0.7).

## How to work

- When a tracking doc does not cover an edge, research it against `lynx/` and
  `lynx-stack/` and cite the files you read. Do not implement from memory.
- A property whose value grammar is missing is a `vendor/stylo` fork patch
  first, then the Rust side. Follow the fork's patch-series workflow.
- Native-Lynx vs web-core behavioral conflicts go to the **user**, not into a
  silent decision. AGENTS.md makes web-core the default resolution; say so
  rather than deciding. Record confirmed divergences in
  `docs/tracking/deviations.md`.
- You cannot spawn subagents. Do the read-only source audit yourself; the main
  session can run `lynx-behavior-researcher` first if it wants one.

## Before finishing

- `./.github/scripts/fmt-check.sh` — never `cargo fmt --all`, it reformats
  `vendor/stylo`.
- `pnpm install --frozen-lockfile` and
  `pnpm --filter reactlynx-test-fixtures build` before any cargo test, clippy,
  or bench run.
- `cargo clippy --all-targets -- -D warnings`.
- `cargo test -p dom --test grammar_values --test grammar_layout --test
  grammar_background --test grammar_border_effects --test grammar_transform
  --test grammar_animation --test cascade_rules --test selectors --test
  custom_properties --test inheritance_computed --test at_rules --test
  media_queries --test supports --test style --test invalidation --test
  style_pool`, plus `cargo test -p bobcat-source` and
  `cargo test -p bobcat-core --test style_sheets`.
- Fork changes: run the `lynx_*` tests under `vendor/stylo/style/tests/`, and
  check `git -C vendor/stylo status` for drift you did not intend.
- The PR body needs before/after Mermaid diagrams
  (`.github/pull_request_template.md`, AGENTS.md "Pull-request descriptions").
