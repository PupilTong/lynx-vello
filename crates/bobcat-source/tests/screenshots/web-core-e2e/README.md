# Renderings that were judged right

One PNG per card in `packages/web-core-e2e-fixtures` whose case someone read,
whose behaviour they decided this engine answers correctly, and whose rendering
this is. `crates/bobcat-source/tests/web_core_e2e.rs` holds each card to its
file; `web_core_e2e/verified.rs` says, per card, what the case tests.

They are **not** web-core's screenshots. Comparing against those directly was
tried and dropped: the two stacks rasterize text differently, and several cases
are about behaviour an image only indirectly shows, so a pixel distance to
chromium answers a question nobody asked. web-core's rendering is evidence
while reading a case, never the criterion.

`<case>.png` is a card's first screen. A card upstream drives further has one
more file per frame, `<case>.<label>.png`, taken after the steps
`web_core_e2e/driven.rs` lists under that label.

A card with no file here is in `web_core_e2e/pending.rs` with a reason, and
`no_pending_case_has_a_golden` fails if one gains a file without moving. That
is the point: a picture of a rendering nobody has judged would be read as a
decision by whoever comes next.

Accepting one is `PRIME_RADIANT_UPDATE_SNAPSHOTS=1` on that single test, after the
reading, in the same change that moves the card into `verified!`.

Cards that show Chinese text depend on the machine's preferred languages. Han
text with no `lang` falls back to the face CoreText picks for the process's
language list, since fontique passes no locale, and two faces differ in line
metrics enough to move every line under them. These files were recorded with
`en-US, zh-Hans-US, ja-US`, and CI sets the same list on its runner before the
tests (`.github/workflows/ci.yml`). A machine with another list renders
`basic-element-text-maxlength`, `-nest-image`, `-tail-color-convert`,
`-word-break` and the two `x-overlay-ng` playground cards differently.
