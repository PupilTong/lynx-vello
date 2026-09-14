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

`__LoadStyleSheet` starts a preload and immediately returns a fresh opaque
handle. The host binding sends a normal stylesheet request directly through
`SourceRequester`; its handle retains the response receiver. Preloading does
not parse text into document rules, mount styles, or wait for IO. A response can
finish without any MTS task running. Unused preloads produce no script errors.

`__AdoptStyleSheet(handle)` synchronously obtains the preload response, mounts
the sheet and returns null. If the response has not arrived, it parks MTS until
the embedder completes it or the view's cancellation token fires. This wait
runs no JS jobs, timers or sibling-view tasks on the group's shared MTS thread.
The resource-owning host continues to service requests through `LynxView::pump`.
There is no nested runtime, stylesheet completion task or deferred adoption queue.

A loader failure throws from `__AdoptStyleSheet` in that same JS call and can be
caught there. An uncaught error follows the existing entry/event error path.
Adopting B never waits for an unused preload A. Sequential adoption calls mount
in their call order, including repeat calls, preserving CSS specificity and
importance. Successful responses are retained by the handle for repeated adoption.
The ordinary element-tree flush/commit publishes the resulting styles.

Collection releases a handle and its response receiver. The existing
`SourceCompletion::is_cancelled` then tells the fetcher that an unused preload
has no consumer. Once mounted, styles belong to the document and survive handle
collection. View release wakes a blocked adoption even if the host retains its
completion; late source results are discarded by the existing resource protocol.

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
Native/web integration verifies the final painted result through ordinary
resource URLs and the embedder's loader.
