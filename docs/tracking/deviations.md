# Known Lynx ↔ W3C behavior deviations

> Status: pending initial research pass — see [README.md](README.md).

This file rolls up every row marked `No`/`Partial` in the W3C-compliant column
across the other tracking files, so the "follow W3C instead of the Lynx quirk"
policy (see [`AGENTS.md`](../../AGENTS.md#standards-policy-w3c-first-lynx-behavior-second))
has one place to check before implementing anything. Each entry should link
back to its source file.

## Known so far

- **`z-index`/stacking context** — Lynx does not implement the CSS
  stacking-context algorithm. Implement the real algorithm instead (see
  [css-layout.md](css-layout.md)).

*(more will be added as the per-domain research files are filled in)*
