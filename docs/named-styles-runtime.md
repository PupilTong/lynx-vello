# Named stylesheet preloading and synchronous adoption

Named CSS uses the same resource loader as startup stylesheets. The embedder
resolves a URL and returns `StyleSheetSource::Text` or
`StyleSheetSource::Preparsed`; JavaScript receives a handle naming the URL in
either case. Core carries no decoded bundle metadata in its view configuration.

## Entry identity and URL mapping

Boot passes its entry response URL to `__BobcatInitEntry` before importing the
application entry. The `__Card__` import in MTS reads that JS binding from
`bobcat:runtime`; there is no native URL getter. The string `"__Card__"` remains
an accepted alias, replaced by JavaScript. A local Lepus chunk load accepts
either the alias or the same entry URL, and its resource URL is built the same
way and in the same place: `chunkURL` beside `styleSheetURL`, appending
`/<encoded-name>.js` with the suffixes preserved. `PageSource` registers the
chunk under exactly that string (`named_chunk_url`).

`__LoadStyleSheet('CSS', bundleName)` appends `/index.css` to the entry or
bundle URL's path. Other section names use `/<encoded-name>/index.css`.
Query and fragment suffixes are preserved after the appended path. For example:

- With entry `https://app.test/main.js`, loading `('CSS', '__Card__')`
  requests `https://app.test/main.js/index.css`.
- Loading `('CSS', 'https://cdn.test/component.bundle?rev=2')`
  requests `https://cdn.test/component.bundle/index.css?rev=2`.

JS constructs the specifier; URL resolution and transport policy remain with
the embedder. It can answer registered URLs, load CSS from a server, or decode a
container and return preparsed styles through the same completion. There is no
additional resource protocol or Rust bundle-name lookup. A complete lazy-component
script loader remains separate work.

## Preload and adopt

`__LoadStyleSheet` returns a plain `{url}` object, whose only content is the
CSS URL. No realm state survives the call either: there is no table behind the
handle, so a handle is exactly as good as the URL it names and one built by
hand works. It sends a `ResourceFetcher::preload_source` hint; core retains no
response, native handle or loading state. A fetcher may ignore the hint.
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
request to the host — so a sibling view that is still loading goes on staging
what arrives and acknowledging the `BeginFrame` an offscreen host is blocked on.
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
using the same URLs as the JS wrapper. Native and web bundles share the
supported descriptor decoder. It lowers StyleRule, KeyframesRule and
FontFaceRule, compiler variables, fallbacks and trailing importance without
reconstructing a whole stylesheet. Malformed or unsupported descriptors are
not registered. Ordinary page styles keep their existing startup resource path;
native/web wire formats and the rkyv 0.7 model are unchanged.

Tests cover boot URL initialization, redirects, escaped section URLs,
text/preparsed equivalence, inert preloading, immediate and repeated adoption,
CSS precedence, collection, synchronous errors and cancellation while waiting.
Resource tests cover shared pending loads, cached failures, registration changes
and scope isolation.
Native/web integration verifies the final painted result through ordinary
resource URLs and the embedder's loader.
