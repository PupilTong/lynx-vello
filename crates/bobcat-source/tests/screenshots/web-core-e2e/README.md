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

A card with no file here is in `web_core_e2e/pending.rs` with a reason, and
`no_pending_case_has_a_golden` fails if one gains a file without moving. That
is the point: a picture of a rendering nobody has judged would be read as a
decision by whoever comes next.

Accepting one is `FLASHBULB_UPDATE_SNAPSHOTS=1` on that single test, after the
reading, in the same change that moves the card into `verified!`.
