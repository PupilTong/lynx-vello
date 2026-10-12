# Named stylesheet preloading and synchronous adoption

Named CSS uses the same resource loader as startup stylesheets. The embedder
resolves a URL and returns `StyleSheetSource::Text` or
`StyleSheetSource::Preparsed`; JavaScript receives a handle naming the URL in
either case. Core carries no decoded bundle metadata in its view configuration.

## Entry identity and URL mapping

The host passes the entry's response URL to `__BobcatInitEntry` before it
completes the module boot imports the application entry as. The `__Card__`
import in MTS reads that JS binding from `bobcat:runtime`; there is no native
URL getter. The string `"__Card__"` remains
an accepted alias, replaced by JavaScript. A local Lepus chunk load accepts
either the alias or the same entry URL, and its resource URL is built the same
way and in the same place: `chunkURL` beside `styleSheetURL`, appending
`/<encoded-name>.js` with the suffixes preserved. `PageSource` registers the
chunk under exactly that string (`named_chunk_url`).

`__LoadStyleSheet(key, bundleName)` appends `/custom_section/<encoded-key>.css`
to the entry or bundle URL's path, the key encoded as a section body's name is
(`form_urlencoded`, a space as `%20`). The rule is one function per side —
`styleSheetURL` in `packages/bobcat-element/src/section-url.ts` and
`named_style_url` in `crates/bobcat-source/src/page.rs`. Query and fragment
suffixes are preserved after the appended path. For example:

- With entry `https://app.test/main.js`, loading `('CSS', '__Card__')`
  requests `https://app.test/main.js/custom_section/CSS.css`.
- Loading `('CSS', 'https://cdn.test/component.bundle?rev=2')`
  requests `https://cdn.test/component.bundle/custom_section/CSS.css?rev=2`.
- Loading `('A &/中', './component.bundle#entry')` requests
  `./component.bundle/custom_section/A%20%26%2F%E4%B8%AD.css#entry`.

JS constructs the specifier; URL resolution and transport policy remain with
the embedder. It can answer registered URLs, load CSS from a server, or decode a
container and return preparsed styles through the same completion. There is no
additional resource protocol or Rust bundle-name lookup. Lazy components are
loaded through `lynx.fetchBundle` and the reference fetcher's
`LazyBundleInstaller`, which registers a fetched container's named stylesheets
at these same URLs ([worker resources](worker-resources-runtime.md)
"Lazy containers").

## Preload and adopt

`__LoadStyleSheet` first asks the host member `hasStyleSheet(url)` whether the
view's fetcher holds a stylesheet at the composed URL. That answer is
`ResourceFetcher::style_sheet_probe`, a `Send + Sync` function the fetcher hands
over at view construction like `fetch_probe`, called synchronously on
`bobcat-main`: no request, no wait, no job parked. The reference fetcher,
`bobcat-resources`, answers from its registry alone — the URL resolved against
its base, then looked up — so an unregistered URL is `false` without any
transport, the network included, being consulted. A container's named sheets
are registered before any of its script can ask (`PageSource::register_with`
at boot, `LazyBundleInstaller::install` before a `loadScript`), so the registry
is the whole answer. A fetcher with no probe answers every URL as held.

When the sheet is not held, `__LoadStyleSheet` answers **`null`** and sends
nothing. Native's `LoadStyleSheet` answers `undefined` for a missing section
(`lynx/core/runtime/lepus/bindings/renderer_functions.cc:963-968`); ReactLynx's
`rLynxPrepareLazyBundleMTS` and its sync lazy path adopt only a handle that is
`!== null`, so `null` is the value that makes them adopt nothing, as native's
`undefined` does there (an `undefined` would reach `__AdoptStyleSheet` and
throw). A container without a `CSS` section is the ordinary case: every
compiled lazy bundle is prepared this way.

When the sheet is held, `__LoadStyleSheet` returns a plain `{url}` object, whose
only content is the CSS URL. No realm state survives the call either: there is
no table behind the handle, so a handle is exactly as good as the URL it names
and one built by hand works. It sends a `ResourceFetcher::preload_source` hint;
core retains no response, native handle or loading state. A fetcher may ignore
the hint.
Preloading neither mounts styles nor waits for IO, and unused preloads produce
no script errors.

Each `__AdoptStyleSheet(handle)` reads `handle.url` in JS — anything else is a
`TypeError` — and makes an ordinary
`SourceRequest::StyleSheet` through the same loader used for startup styles.
It synchronously mounts the response and returns null. Only that call holds a
response receiver. If the response has not arrived, the adoption parks the job
it is running in until the embedder completes it or the view's cancellation
token fires — a `JsThread::wait`, biased on the view's own token so a release
ends the wait rather than waiting out an answer that will never come.

What that wait does and does not run follows from `bobcat-main` being a
`JsThread`: JavaScript runs in jobs, and the jobs are one FIFO, so no promise
job, no timer callback and no sibling view's entry runs before this adoption
returns. The thread's *tasks* keep running throughout — the command consumers,
the boot futures, the clock tasks, and the notice traffic that carries this very
request to the host. What a task receives meanwhile waits as a job behind the
adoption: a sibling view's command burst, and with it the acknowledgement of
the frame post an offscreen host is blocked on, runs only once this adoption
returns.
The resource host continues servicing requests through `LynxView::pump`.

The reference fetcher, `bobcat-resources`, owns a stylesheet response cache keyed
by resolved URL. Preload and ordinary requests share pending work and reuse its
completed result, including failures. The cache lives in a `Resources` scope;
clones share it and `new_scope` starts empty. Registering, replacing or removing
a registered URL invalidates its response. Results from an invalidated load cannot
replace the new cache entry. Preparsed registrations are already resident and are
served directly. Other embedders choose their own cache and preload policy.

A loader failure throws from `__AdoptStyleSheet` in that same JS call and can be
caught there. An uncaught error follows the existing entry/event error path.
With the reference fetcher that failure is no longer how a missing container
section shows up — `__LoadStyleSheet` answered `null` for it — but a handle
built by hand, or one whose registration was removed after it was handed out,
still fails here.
Sequential adoption calls mount in call order, including repeats, preserving
CSS specificity and importance. Every call requests its URL again; core does
not cache the response behind the JS handle. The ordinary element-tree
flush/commit publishes the resulting styles.

Collecting a handle releases nothing but the object: it sends no native release
or cancellation, and preload lifetime belongs to the resource scope. Mounted
styles belong to the document. View release wakes a blocked adoption even if
the host retains its completion; late responses are discarded by the existing
resource protocol.

## Source ownership and validation

`PageSource` registers named compiler CSS with `Resources::register_style_sheet`
using the same URLs as the JS wrapper (`crates/bobcat-source/src/custom_style.rs`
`named_style_sheets`). A named sheet comes from one of three places, in this
precedence: the `.web.bundle` `CustomSectionsStyleInfo` section (label 7,
lynx-stack#4138), one pre-parsed `StyleInfo` per section lowered by
`lower_style::to_preparsed_style_sheet`; the **legacy** JSON
`{encoding:"CSS", content:{ruleList}}` descriptor in `CustomSections`, which
encoders before lynx-stack#4138 wrote and which is kept for those bundles; and
a `.lynx.bundle`'s CSS custom sections, which `native::decode` turns into the
same per-name `StyleInfo`s. Both paths lower StyleRule, KeyframesRule and
FontFaceRule, compiler variables, fallbacks and trailing importance without
reconstructing a whole stylesheet. Malformed or unsupported legacy descriptors
are not registered, and a label-7 section that fails validation fails the
bundle's decode. Ordinary page styles keep their existing startup resource
path.

Tests cover boot URL initialization, redirects, escaped section URLs,
text/preparsed equivalence, inert preloading, immediate and repeated adoption,
CSS precedence, collection, synchronous errors and cancellation while waiting.
Resource tests cover shared pending loads, cached failures, registration changes
and scope isolation.
Native/web integration verifies the final painted result through ordinary
resource URLs and the embedder's loader.
