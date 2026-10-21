# Text test fixtures

`Ahem.ttf` is the CSS Working Group's deterministic test font, vendored from
the web-platform-tests repository at `fonts/Ahem.ttf` (SHA-256
`b719ecb31c5b21fc573c03f6421c74ac63c271a5a3ff841e34f9705fb94b8448`).
Every printable glyph is one em wide, which makes text-layout geometry exact
and independent of fonts installed on the test host.

The web-platform-tests project distributes this fixture under its 3-Clause
BSD license. Copyright web-platform-tests contributors. See
<https://github.com/web-platform-tests/wpt/blob/master/LICENSE.md>.

`Roboto-Regular.ttf` is the second text fixture, vendored from
<https://github.com/googlefonts/roboto-2> at `src/hinted/Roboto-Regular.ttf`
(SHA-256
`56a45233d29f11b4dfb86d248e921939d115778f87325e7ae8cc108383d6664d`).
Ahem's glyphs are solid em squares, which is what makes it exact — and also
what makes it useless for reviewing *rendering*: a screenshot of Ahem text
says nothing about antialiasing, glyph outlines, synthesis, or how decorations
sit against real letterforms. Roboto covers that half. It is committed rather
than resolved from the host so the screenshot goldens stay identical across
machines; nothing in the engine depends on it, and it is Android's system
font, so it is also representative of what Lynx apps actually render with.

Google distributes Roboto under the Apache License 2.0, reproduced verbatim
in `Roboto.LICENSE.txt`. Copyright 2015 Google Inc.
