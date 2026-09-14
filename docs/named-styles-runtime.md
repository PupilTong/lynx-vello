# Named stylesheet preloading and synchronous adoption

Named CSS uses the same resource loader as startup stylesheets. The embedder
resolves a URL and returns `StyleSheetSource::Text` or
`StyleSheetSource::Preparsed`; JavaScript receives an opaque handle in either
case. Core carries no decoded bundle metadata in its view configuration.

## Entry identity and URL mapping

Boot passes its entry response URL to `__BobcatInitEntry` before importing the
application entry. The `__Card__` import in MTS reads that JS binding from
`bobcat:runtime`; there is no native URL getter. The string `"__Card__"` remains
an accepted alias, replaced by JavaScript. Local Lepus chunk lookup accepts
either the alias or the same entry URL.

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

`__LoadStyleSheet` returns a fresh opaque JS object whose only associated data
is the CSS URL. It sends a `ResourceFetcher::preload_source` hint; core retains
no response, native handle or loading state. A fetcher may ignore the hint.
Preloading neither mounts styles nor waits for IO, and unused preloads produce
no script errors.

Each `__AdoptStyleSheet(handle)` reads the URL in JS and makes an ordinary
`SourceRequest::StyleSheet` through the same loader used for startup styles.
It synchronously mounts the response and returns null. Only that call holds a
response receiver. If the response has not arrived, MTS parks until the embedder
completes it or the view's cancellation token fires. This wait runs no JS jobs,
timers or sibling-view tasks on the group's shared MTS thread. The resource host
continues servicing requests through `LynxView::pump`.

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

Collecting a JS handle releases only its URL association. It sends no native
release or cancellation; preload lifetime belongs to the resource scope.
Mounted styles belong to the document and survive handle collection. View
release wakes a blocked adoption even if the host retains its completion;
late responses are discarded by the existing resource protocol.

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
