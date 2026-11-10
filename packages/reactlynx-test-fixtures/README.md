# ReactLynx test fixtures

This private pnpm workspace owns the JSX/CSS/JS sources used by Bobcat's
compiled ReactLynx integration tests, decoder tests and benchmarks. The five
`basic-*` cards, the `react-frame*` cards (web-core-e2e's `api-frame-src`,
`api-frame-inner`, `api-frame-data`, `api-frame-global-props`,
`api-frame-auto-height` and `api-frame-data-update`, whose `global-props`
twin `react-frame-global-props-update` has no upstream case, nor do
`react-frame-tap` and its `react-frame-tap-inner`) and the
`react-set-shared-data`/
`react-get-shared-data` pair (web-core-e2e's `api-setSharedData`/
`api-getSharedData`) come from lynx-stack; see `NOTICE.lynx-stack` for
provenance.

## Build

```sh
pnpm install --frozen-lockfile
pnpm --filter reactlynx-test-fixtures build
```

`rsbuild.config.js` declares the entries, independent output directories and
`pluginReactLynx({ engineVersion: '4.1.0' })`. Other ReactLynx options keep their
defaults. The package scripts invoke the public `rsbuild build` CLI once in
production mode and once in development mode. No script creates a compiler or
imports an internal build-tool entry point.
Both commands set `NODE_ENV` explicitly so the fixture matrix and compiler
use the same mode. A third step, `build:bytecode`, runs
`scripts/bytecode-bundles.ts` with Node after both; see
[PrimJS bytecode bundles](#primjs-bytecode-bundles).

The default build produces eight native production pages, three native development
variants and twenty-eight web pages. Each native environment has its own
compilation, so its lazy chunks cannot be shared with another test page.

| Environment | Fixture behavior |
| --- | --- |
| `lynx-react-lazy` | Lazy/Suspense, a BTS effect and child CSS |
| `lynx-react-lazy-sync` | First-screen sync import and event-triggered import |
| `lynx-react-lazy-nested` | Nested async/sync imports, shared BTS module and CSS |
| `lynx-react-reload` | State, effect cleanup, host/BTS reload and entry counter |
| `lynx-react-data-processor` | Synchronous default/named processors and data updates |
| `lynx-react-global-props` | Reactive global props, initial state and clicks |
| `lynx-react-list` | A forty-cell `<list>`, and a tap that removes three cells and appends one |
| `lynx-react-native`, `web-react-bts-query` | Ref fields, typed dataset, scoped query and native props |
| `web-basic-bindtap` | Event delivery and state updates |
| `web-basic-class-selector`, `web-basic-performance-large-css` | CSS decoding and rendered cards |
| `web-basic-mts-run-on-main-thread`, `web-basic-mts-run-on-background` | Worklets and main-thread refs |
| `web-react-scroll-timeline` | A row revealing along its list's `animation-timeline: scroll()` |
| `web-react-viewpager` | A `<viewpager>` of four coloured pages starting on the second (`select-index`), and a tap whose selector query `selectTab`s the fourth |
| `web-react-scroll-coordinator` | A `<scroll-coordinator>` with a translucent toolbar over a red header and a slot whose `<scroll-view>` holds eight 100px items; a forward drag in the items folds the header first |
| `web-react-swiper` | Six `<x-swiper>`s of three coloured items: `current`, `vertical`, `mode='coverflow'`, a second `current`, `indicator-dots={false}` and custom dot colours |
| `web-react-swiper-scroll-to` | An `<x-swiper>` of four coloured items filling the page, starting on the second (`current`), and a tap whose selector query `scrollTo`s the fourth |
| `web-react-refresh-view` | Two `<x-refresh-view>`s side by side, each a red header (author `position: absolute`), a yellow `scroll-view` of green items and a red footer; the right one `enable-refresh={false}` |
| `web-react-dialog` | A red page whose tap opens a `<dialog>` with `showModal()` (selector query `invoke`) over a yellow author `::backdrop`; a tap on its green `#close` calls `close()`, a tap on the backdrop `requestClose()`, and `bindclose`/`bindcancel` recolour the page |
| `web-react-overlay` | A red page whose tap sets `visible` on an `<overlay>` carrying an author `display: flex` and a translucent green background, neither of which may show: its one child, a yellow 200px panel at the viewport's top-left, paints above the page and turns magenta on `bindshowoverlay`, which also turns the page orange; a tap on its green `#close` (`catchtap`) clears `visible`, and `binddismissoverlay` turns the page blue |
| `web-react-overlay-pass-through` | A red page with a blue 60px `#below` square at its bottom-right; a tap sets `visible` on an `events-pass-through` `<overlay>` whose one child is a yellow 120px panel at the top-left. `#below` counts its taps by colour (cyan after one with `showoverlay` seen, magenta after two), so a second tap reaches it only through the pass-through overlay |
| `web-react-svg` | A white 240px page in bands: two `<svg content={...}>`s sized by CSS (a `<linearGradient>` fill through `url(#fade)` with a green band and a yellow `<path>` triangle, and a blue card with centred white `<text>`); an unsized `<svg src>` of a bundled `shape.svg` at its natural 80 by 40; `<image mode="aspectFit">` of the same `shape.svg` letterboxed on magenta; a `<view>` tiling `tile.svg` with `background-repeat: repeat`. No `<svg>` has children. Both assets are inlined as `data:` URLs; `crates/bobcat-source/tests/reactlynx_runtime.rs` samples the pixels |
| `web-react-markdown` | An `<x-markdown>` (web-core's tag) rendering web-core's `basic.html` content followed by a `[link]` paragraph and a `data:` PNG image, with a structured `markdown-style={{link: …, inlineCode: …}}`, `content-id="case-1"` and `bindlink`/`bindparseEnd`/`bindimageTap`; a sibling `<markdown>` (native's tag) with plain content; a mount effect whose selector query `invoke`s `getContent`. The background thread logs each event as `markdown-card <name> <detail JSON>`, and `#status` names them. No stylesheet, every style inline. `crates/bobcat-core/src/main/tree/markdown/card_tests.rs` reads its shadow tree and taps the link |
| `web-react-frame`, `web-react-frame-inner` | A red page whose flex view puts a 100px `<frame src="./react-frame-inner.web.bundle">` at the page's top-left. The inner card is a blue 100px view holding `frame:ready` (green once its `label` or `message` reads `after`), then upstream's `data:{initData.label}` and `global:{lynx.__globalProps.message}` texts as two 20px rows below it (140px of content in all, which a 100px frame clips); its component body logs `react-frame-inner-rendered`, `react-frame-inner-data:<label>` and `react-frame-inner-global:<message>` on each of the card's threads (the frame realm's MTS first screen, the card's own BTS hydration and every BTS re-render), and its mount effect logs `react-frame-inner-hydrated` once, from the BTS. Ported from lynx-stack's web-core-e2e `api-frame-src`/`api-frame-inner`. `crates/bobcat-source/tests/frame.rs` serves the inner bundle where `src` resolves and samples the pixels |
| `web-react-frame-data` | `react-frame`'s page with `data={{ label: 'from-data' }}` and `global-props={{ message: 'from-global-props' }}` on the frame, which the inner card boots with as its init data and `lynx.__globalProps`. web-core-e2e's `api-frame-data` and `api-frame-global-props` in one host; `crates/bobcat-source/tests/frame.rs` reads the inner card's logs |
| `web-react-frame-auto-height` | A red page whose `display: flex` column holds a 300px-wide green `<frame auto-height src="./react-frame-inner.web.bundle">` with no height, which takes the inner card's 140px content height (a column does not stretch it). Adapted from web-core-e2e's `api-frame-auto-height`, plus the green background that shows the frame's box; `crates/bobcat-source/tests/frame.rs` samples the pixels |
| `web-react-frame-data-update` | web-core-e2e's `api-frame-data-update`: a red page whose `display: flex` column puts a 100px `<frame src="./react-frame-inner.web.bundle" data={{ label }}>` at the top-left and a yellow 100px `#update-frame-data` square right below it, (0, 100) to (100, 200), whose tap sets `label` from `before` to `after`; a mount effect logs `react-frame-data-update-hydrated` from the BTS. The inner card turns green on its BTS's re-render; `crates/bobcat-source/tests/frame.rs` taps the square and samples the pixels |
| `web-react-frame-global-props-update` | `web-react-frame-data-update` for `global-props={{ message }}`, with an `#update-frame-global-props` square and a `react-frame-global-props-update-hydrated` log; no upstream case. The inner card turns green on its BTS's reactive global-props re-render; `crates/bobcat-source/tests/frame.rs` taps the square and samples the pixels |
| `web-react-frame-tap`, `web-react-frame-tap-inner` | No upstream case: a tap inside a `<frame>` runs the card's own handlers and the host's. A red page whose `display: flex` column puts a yellow 100px spacer at the top-left and, below it, a `#frame-wrapper` view whose `bindtap` logs `react-frame-tap-host:<e.target.id>:<detail.y>` around a 100px-wide, 200px-tall `<frame id="target" src="./react-frame-tap-inner.web.bundle">`, (0, 100) to (100, 300); a mount effect logs `react-frame-tap-hydrated` from the BTS. The inner card is two 100px views in a column: a blue `#tap-me` whose `catchtap` logs `react-frame-tap-inner-at:<detail.x>,<detail.y>` and counts the tap in state on the card's own BTS (green from then on), and a magenta `#tap-me-on-main-thread` whose `main-thread:bindtap` worklet logs `react-frame-tap-inner-main-thread-at:<detail.x>,<detail.y>` and paints itself green with `setStyleProperty`; its component body logs `react-frame-tap-inner-count:<n>` on each thread's render and its mount effect `react-frame-tap-inner-hydrated` once. `crates/bobcat-source/tests/frame.rs` taps each view and samples the pixels |
| `web-react-set-shared-data`, `web-react-get-shared-data` | web-core-e2e's `api-setSharedData` and `api-getSharedData`, copied: the first card's effect stores `green` under `color` with `lynx.setSharedData` and paints its own `getSharedData`; a tap on the second, an orange 100px view, paints what `getSharedData('color')` reads. Booted as two views of one group by `crates/bobcat-source/tests/reactlynx_runtime.rs`, which is web-core-e2e's `api-shared-context` |
| `web-css-custom-sections` | A 100px red `.page` card whose bundle also carries two `encoding: 'CSS'` custom sections, `CSS` (`CSS.css`) and `A &/中` (`named.css`). No ReactLynx build writes such a web card today (a lazy bundle's CSS goes into its `StyleInfo` on the web target), so `scripts/css-custom-sections.ts` adds them through the template plugin's public `beforeEncode` hook; `@lynx-js/web-core/encode` then writes them as the `CustomSectionsStyleInfo` section (label 7, lynx-stack#4138). Decoded by `crates/bobcat-source/tests/decode_web_bundle.rs` |

Development builds select `lynx-react-reload`, `lynx-react-global-props` and
`lynx-react-lazy-nested`, retaining the compiler's default HMR client and asset
prefix. Select a single environment through the CLI:

```sh
pnpm --filter reactlynx-test-fixtures build:production --environment lynx-react-lazy
pnpm --filter reactlynx-test-fixtures build:development --environment lynx-react-reload
pnpm --filter reactlynx-test-fixtures build:production --environment web-react-bts-query
```

`pnpm test:type` includes the configuration and TypeScript build hook. ReactLynx
compiles the fixture inputs. Processors return their data synchronously; the
fixture does not require nested Promise jobs to run before that return value
is consumed.

## Outputs

Outputs live only in ignored `dist/`; compiled bundles and provenance must not
be committed. Native pages retain `dist/<fixture>/<fixture>.lynx.bundle` and
`dist/<fixture>/async/*`. Development directory names append `-development`.
Web pages are `dist/<fixture>/<fixture>.web-<fixture>.bundle`.

The shared `scripts/lynx-bytecode.ts` hook disables BTS manifest bytecode without
changing the native page layout. The existing fixture-only adapter in
`scripts/source-bundles.ts` registers Rsbuild completion hooks. For native
pages it reads the compiler input retained by `DEBUG=lynx` and repacks the
original MTS/BTS source into external custom sections using `@lynx-js/tasm`.
It replaces the page's bytecode container, preserves emitted lazy bundle bytes
and leaves web output unchanged, except for `web-css-custom-sections`, whose
two custom sections `scripts/css-custom-sections.ts` adds before encoding.
`DEBUG=lynx` also retains MTS source inside lazy bundles; these fixtures
exercise source evaluation, not native bytecode.

The encoder is pinned to `@lynx-js/tasm@0.0.53`. A workspace override keeps the
compiler and repacker on that same version; the previous encoder cannot target
engine 4.1.0. The hook still relies on the pinned compiler's debug `tasm.json`
format, which should be checked when upgrading the toolchain.

Each environment writes `dist/<output-directory>.provenance.json` with its actual
source dependencies, compiler versions and bundle hashes. Native records also
include source-section hashes, the original page hash and its public path.
Chunk names may change with paths, compiler versions and development hashes;
consumers should use the provenance map instead of guessing filenames.

Historical 3.5/native-example and QueryComponent generators are removed. This
workspace targets the selected 4.1.0/default-options fixtures only.

The final build hook writes `dist/index.rs` from all provenance records.
`fixtures.rs` includes that generated registry for native/Wasm tests and
benchmarks, so consumers use the compiler's actual chunk names and public paths.
Run the fixture build before `cargo test`, `cargo clippy --all-targets` or
benchmark compilation; CI does the same. Generated JS, bundles and the registry
stay out of version control.

Lazy fixtures remain available as source inputs for later work. Their external
bundle loading is not part of the current runtime integration suite.

## PrimJS bytecode bundles

`scripts/bytecode-bundles.ts` (`pnpm build:bytecode`, the last step of
`build`) writes native bundles whose JavaScript is PrimJS bytecode, for tests
of the converter in `docs/primjs-bytecode-conversion-design.md`. It calls
`@lynx-js/tasm`'s native encoder directly and stops with an error on any
platform where that encoder is not shipped (it is shipped for macOS and x64
Linux). Everything goes to `dist/bytecode/`, which the step deletes and
rewrites; the outputs of the Rsbuild steps are not changed.

### Language corpus

`bytecode-corpus/` holds one plain script per language topic. Every program
follows one protocol: it assigns a string to `globalThis.__result`, either
synchronously or from a promise job, so a harness runs the script, drains
pending jobs and reads that global. Programs use no host API (no `console`,
timers, `lynx`, modules or `eval`) and report caught errors by `name`, never
by message.

Each program is encoded four times, as `dist/bytecode/corpus/<program>/<variant>.lynx.bundle`.
Every bundle is a fiber-architecture LepusNG page (`enableFiberArch`,
`useLepusNG`, `dsl: 'react_nodiff'`) with
`experimental_encodeQuickjsBytecode: true`, and carries the program twice: as
the main-thread script (`ROOT_LEPUS`) and as the background script
`/app-service.js` (`JS_BYTECODE`, inside the 24-byte packed header). The
build checks the header of both on every run.

| Variant | `targetSdkVersion` | `debugInfoOutside` | `ROOT_LEPUS` | `JS_BYTECODE` |
| --- | --- | --- | --- | --- |
| `sdk-2.13` | `2.13` | `true` | version 1; debug blocks name the atom `__lepusNG_function_id__` and a function id, no pc2line | version 1, no debug blocks (`LEPUS_EVAL_FLAG_STRIP`) |
| `sdk-2.14` | `2.14` | `true` | version 9, `NEW_DEBUGINFO_FLAG` | version 9, `NEW_DEBUGINFO_FLAG` |
| `sdk-4.1` | `4.1` | `true` | version 9, `NEW_DEBUGINFO_FLAG` and `STRIP_VARINFO_FLAG` | as `sdk-2.14` |
| `inline-debug` | `4.1` | `false` | version 1, inline debug info (filename, line, pc2line) | as `sdk-2.14` |

Why these are all the forms tasm 0.0.53 writes, in the lynx sources:

- The main-thread compile sets the PrimJS context's target SDK only when
  `debugInfoOutside` is on (`core/runtime/lepus/bytecode_generator.cc`,
  `GenerateBytecodeForQuickContext`), and PrimJS writes version 9 only when a
  target SDK of 2.14 or later is set (`JS_IsNewVersion`). So version 9 always
  comes with external debug info, and without `debugInfoOutside` the main
  thread is version 1 with inline debug info at any target SDK.
- The background compile decides by target SDK alone
  (`template_binary_writer.cc`, `EncodeJsBytecode`): below 2.14 stripped,
  otherwise external debug info. `STRIP_VARINFO_FLAG` does not appear there
  at 4.1: PrimJS sets it only for a LepusNG context (`JS_ShouldStripVarInfo`),
  and the bytes show the background compile does not use one.
- `enableOptLepusNGBytecode` (`LEPUS_SetOptLepusNGPackageSize`) produced the
  same bytes as without it for every corpus program in every variant, so it is
  not a variant.

Programs the PrimJS compiler in tasm 0.0.53 rejects are not in the corpus: `#x in obj`
(unexpected token), class `static` blocks (invalid property name), `&&=`,
`||=` and `??=` (unexpected token), the RegExp `d` and `v` flags (invalid flags) and
`\p{...}` property escapes (invalid escape). A call in a class field
initialiser (`a = f();`, instance or static) crashes the encoder process with
SIGSEGV, so `class-fields.js` and `class-static-fields.js` avoid one.

### ReactLynx fixtures

For every native environment (`target: 'lynx'` in its provenance), the step
re-encodes the retained `.lynx/<entry>/tasm.json` and checks that the bytes
equal the page Rsbuild emitted (`nativePageSha256`) before
`scripts/source-bundles.ts` replaced it. That page, with main-thread bytecode
and background source, is written as
`dist/bytecode/fixtures/<fixture>/<entry>.lynx.bundle`; the same input with
`experimental_encodeQuickjsBytecode: true` is written as
`<entry>.js-bytecode.lynx.bundle`, with a `JS_BYTECODE` section in place of
`JS`. Lazy chunks are not re-encoded; they are the ones `dist/index.rs` lists.

### Rust registry

The step writes `dist/bytecode/index.rs`, which `bytecode.rs` includes as
`PROGRAMS` (name, source text, one bundle per `Variant`) and `FIXTURES` (name,
both pages). A test includes it the way it includes `fixtures.rs`:

```rust
#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
mod bytecode;

for program in bytecode::PROGRAMS {
    for variant in bytecode::Variant::ALL {
        let bundle: &[u8] = program.bundle(variant);
        // run program.source, run the converted bundle, compare __result
    }
}
```
