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

Cards that show Chinese text depend on one macOS font asset. Han text with no
`lang` falls back to PingFang SC, an on-demand asset
(`/System/Library/AssetsV2/com_apple_MobileAsset_Font8/…/PingFang.ttc`); where
it has not been downloaded, CoreText answers with the private `.PingFang UI`
face, which fontique cannot use, and the text falls back to Heiti SC, whose
line metrics move every line under it. These files were recorded with
PingFang SC, and CI downloads it before the tests
(`.github/scripts/download-pingfang.swift`). Without it,
`basic-element-text-maxlength`, `-nest-image`, `-tail-color-convert`,
`-word-break` and the two `x-overlay-ng` playground cards render differently.
