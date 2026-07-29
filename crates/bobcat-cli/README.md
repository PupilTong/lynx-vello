# bobcat-cli

`bobcat-cli` builds the `bobcat` executable. It accepts a local Lynx web bundle
as a `file:///` URL and renders it through `bobcat-quickjs` → `lynx-element` →
`dom`/`hughie` → `pulsar`.

```sh
# Native macOS window
bobcat -i file:///absolute/path/to/card.web.bundle

# Paced headless session
bobcat -i file:///absolute/path/to/card.web.bundle --headless --vsync 60
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

Headed mode is macOS-only today. Headless rendering is platform-neutral and
uses whatever GPU adapter Vello/wgpu can acquire. Only screenshots synchronize
the CPU for RGBA readback; normal frames reuse the scene, Vello renderer, GPU
target, and scratch allocations.

Current `bobcat-quickjs` limits still apply. In particular, most real
ReactLynx bundles currently stop at an unimplemented main-thread global before
rendering, and decoded `StyleInfo` rules are not yet lowered into author CSS.
The CLI reports the former as an error and the latter as an explicit warning.
