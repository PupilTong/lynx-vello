# crates/bobcat-source

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/bobcat-source`. Paths in backticks are relative to the repository root.

The single owner of Lynx source parsing and adaptation: the always-available
`ZipSource` API for bounded ZIP decoding, entry selection through `PageSource`,
and resource registration. Native and Wasm embedders share that API; IO and
resource-scope lifetime stay the host's. The crate depends on `bobcat-core`
and `vant` and on no fetcher: every registration is written through
`vant::registry`'s traits (`&impl Register<PreparsedStyleSheet>`, or
`&impl RegisterInBackground<PreparsedStyleSheet>` for the deferred path), which
`bobcat_resources::Resources` implements, and `bobcat-resources` is a
dev-dependency its tests boot cards with.

`xml` is the zero-dependency, zero-copy restricted envelope parser
(`engine-version`, `thread="main"` / `thread="background"`), retaining UTF-16
and UTF-8 error offsets. `web` decodes `SDRA WROF` on the unchanged rkyv 0.7
wire model. `native` decodes source-based flexible external `.lynx.bundle`
files into that same model and can explicitly encode a web bundle; it rejects
real QuickJS/Lepus bytecode rather than executing or decompiling it. Named
external modules are preserved and acquire no invented page root.
`PageSource::from_native_bundle` requires an explicit entry name, `from_bytes`
a `root` for binary page inputs. A `.web.bundle` is read in place: bodies are
borrowed and copied once, into their registered text, and each `StyleInfo` is
lowered from its validated archive through the `StyleInfoRead` traits, never
deserialized into an owned tree on that path. `PageSource::defer` scans only
the section table, so a view boots from its `DeferredPage::view_sources` while
`register_in_background` decodes and registers the bundle through
`RegisterInBackground::register_in_background`
(`docs/source-architecture.md`, "Booting while the bundle is decoded").

**The import wrappers of a card's bodies are this crate's.** The engine adds
nothing to a script, so every card body `bobcat-source` registers carries the
names its realm gives a card, on the body's own first line, so every line of
the body keeps its number (a column on the body's first line is offset by the
prefix's length): an MTS body — a container's root Lepus script, an XML
main-thread script — prefixed with `bobcat_core::MTS_CHUNK_PREAMBLE`
(`mts_entry_source`), and a BTS body prefixed with
`bobcat_core::BTS_CHUNK_PREAMBLE` in the shape its compiler calls for
(`bts_module_source`). An XML
background-thread script is a `.web.bundle`'s `CommonJS` shape, because
web-core runs it as its bundle's `/app-service.js`, through its chunk wrapper.
A container's `main-thread`/`*__main-thread` section — a Lepus chunk or a
string custom section of that name — is the MTS preamble, `export default `
and the body, because `ReactLynx` calls the expression's value
(`lynx.loadScript('main-thread', …)(entry)`). Any other named Lepus chunk is
registered verbatim, since the realm compiles it as a function body over those
same names; a `.json` body is verbatim too, its URL telling the loader to
parse it.

**One `ContainerSources` for every container.** A `.web.bundle` or
`.lynx.bundle` is registered by one path whoever asked for it: a page
(`PageSource` holds one), a `<frame>`'s main bundle, and a container
`lynx.fetchBundle` fetched (`BundleInstaller`, the one `ContainerInstaller`)
all go through `ContainerSources::from_bytes` (or `from_native_bundle` with
an explicit native entry, aliased to `root.js` and registered under its own
name too). With `U` the container URL, `P` its path (one trailing `/`
dropped) and `S` its `?#` suffix, every name encoded by `form_urlencoded`
with a space as `%20` after one leading `/` is stripped:

| What | URL | Body |
| --- | --- | --- |
| Lepus `root`, or the explicit native entry | `P/root.js S` | `mts_entry_source` |
| Lepus chunk or string section `main-thread`/`*__main-thread` | `P/<name>.js S` | preamble + `export default ` + body |
| any other Lepus chunk | `P/<name>.js S` | verbatim |
| any other string custom section | `P/<name>.js S` | `.json` verbatim, else `bts_module_source` |
| manifest path `p` | `P/<p> S` (no `.js`, `?`/`#` in `p` escaped) | the same choice by name |
| named stylesheet `k` | `P/custom_section/<k>.css S` | `PreparsedStyleSheet` |
| the container's non-empty StyleInfo | `P/style-info.css S` | lowered `PreparsedStyleSheet` |
| BTS boot (every container with an entry) | `U` with fragment `#background-thread` | `background_boot_source` |

`packages/bobcat-element/src/section-url.ts` writes the same strings for both
realms. The entry is the Lepus chunk literally named `root`; a container
without one (a lazy bundle) has no entry and no boot, and nothing guesses a
sole `*__main-thread` as one. A page's view names `<U>/root.js` as its entry,
`U#background-thread` as its BTS entry, `style-info.css` as its only listed
sheet and `U` as its `ViewSources::card_url`; a fetched container's StyleInfo
is registered, never asked for by a lazy component, and adopted only by a
`<frame src>` boot into the frame's shadow root. Scripts are collected per URL: one text at
one URL deduplicates, two texts — or a sheet URL a script also reaches — is
`SourceError::ConflictingSection`, and a URL that cannot be a base is
`SourceError::OpaqueContainerUrl`. The native decoder files each custom
source section `n` as both `custom_sections[n]` and manifest `/n`, so its
body is at `P/n.js` and at `P/n`: two modules, evaluated twice if a card asks
by both. Re-registering `U` with new bytes replaces what the new container
names and leaves any URL only the old one had. A page's non-root Lepus chunk
named `*__main-thread` takes the section shape, so `__LoadLepusChunk` on it
reports a `SyntaxError`, and a source-based `.lynx.bundle` loaded with no
explicit entry (a `<frame>`'s) has no `root.js`. These accepted consequences
are listed in `docs/source-architecture.md` "One container URL rule". XML
(`bobcat-memory://lynx-xml/*`, the browser's response fragments) and ZIP's
`bobcat-memory://archive/` are outside this.

`PageSource`, browser response registration, shared StyleInfo lowering and all
three parsers are always available: the crate has no Cargo feature flags, so
every embedder including Wasm depends on the complete crate. IO and view
construction stay with the embedder; the browser's `loadLynxXml` accepts only
XML responses. Native XML keeps strict UTF-8 and private memory URLs; the
browser keeps replacement decoding, final-response fragment URLs and host
PageConfig. The two register the XML background script under different
URLs: native under `bobcat-memory://lynx-xml/app-service.js`, the browser's
`register_lynx_xml_response` at `<final-response-URL>#background-thread`, which
keeps the response URL as the base for that script's own relative imports.
Either way the URL is named in `ViewSources::background_entry`, so the view's
BTS Worker imports it. See `docs/source-architecture.md` for boundaries,
migration and parser resource bounds.
