# bobcat-cli

`bobcat-cli` builds the `bobcat` executable. It accepts either a local Lynx
`.web.bundle` or a raw Lynx XML source document as a `file:///` URL and
privately composes `bobcat-core/quickjs` →
`dom`/`hughie`. The CLI is an independent product;
its renderer is not exported from `bobcat-core` as an embedder façade.

```sh
# Native macOS window
bobcat -i file:///absolute/path/to/card.web.bundle

# Paced headless session (optionally at a HiDPI scale)
bobcat -i file:///absolute/path/to/card.web.bundle --headless --vsync 60 --dpr 2

# Raw Lynx XML uses the same renderer and prompt
bobcat -i file:///absolute/path/to/card.lynx.xml --headless
```

Input selection is content-based rather than extension-based. After leading
ASCII whitespace and UTF-8 byte-order marks are skipped for sniffing, a `<`
selects Lynx XML; every other input goes through the web-bundle decoder. XML is
then decoded as strict UTF-8 and parsed with the restricted Lynx XML grammar,
including the required `engine-version` root attribute and
`<script thread="main">` section. Its optional `<style>` body is mounted as
author CSS before the required main-thread script runs. XML uses
`defaultDisplayLinear = false`,
`defaultOverflowVisible = false`, and `enableCSSSelector = true`.

Both modes expose a small debugger-style prompt. Screenshots are captured from
the live session rather than by a one-shot startup option:

```text
(bobcat) pause
Frame clock paused.
(bobcat) frame
Rendered one frame.
(bobcat) screenshot captures/current.png
Saved screenshot to captures/current.png.
(bobcat) continue
Continuing at 60 Hz.
(bobcat) quit
```

Run `bobcat --help` for all startup options and enter `help` at the prompt for
all runtime commands.

Headed mode is macOS-only today; it derives the device-pixel ratio from the
window, while headless mode takes it from `--dpr` (default 1). Only
screenshots read pixels back to the CPU; normal frames reuse the scene, Vello
renderer, GPU target, and scratch allocations, skip the GPU entirely while
the document is unchanged, and wait for each submitted frame so a clock that
outpaces the GPU cannot pile up work.

Current `bobcat-core` QuickJS limits still apply. In particular, most real
ReactLynx bundles currently stop at an unimplemented main-thread global before
rendering. Component-scoped bundle CSS is currently mounted globally and is
reported as a warning. A present XML background-thread section is retained at
the conventional `/app-service.js` URL (including a present empty section),
but background-thread JavaScript is not executed yet; the CLI warns explicitly
when such a section is present.
