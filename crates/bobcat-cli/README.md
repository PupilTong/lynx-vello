# bobcat-cli

`bobcat-cli` builds the `bobcat` executable. It accepts a local Lynx web bundle
as a `file:///` URL and privately composes `bobcat-core/quickjs` →
`dom`/`hughie`. The CLI is an independent product;
its renderer is not exported from `bobcat-core` as an embedder façade.

```sh
# Native macOS window
bobcat -i file:///absolute/path/to/card.web.bundle

# Paced headless session (optionally at a HiDPI scale)
bobcat -i file:///absolute/path/to/card.web.bundle --headless --vsync 60 --dpr 2
```

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
rendering, and decoded `StyleInfo` rules are not yet lowered into author CSS.
The CLI reports the former as an error and the latter as an explicit warning.
