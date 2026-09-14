# Named stylesheet loading through resource URLs

Named CSS uses the same resource loader as startup stylesheets. The embedder
resolves a URL and returns `StyleSheetSource::Text` or
`StyleSheetSource::Preparsed`; JavaScript receives an opaque handle in either
case. Core carries no decoded bundle metadata in its view configuration.

## Entry identity and URL mapping

The `__Card__` import in MTS contains the entry response URL already supplied
by the resource loader. It is also exported by `bobcat:runtime`. The string
`"__Card__"` remains an accepted alias; JavaScript replaces it with that URL.
Local Lepus chunk lookup accepts either the alias or the same entry URL.

`__LoadStyleSheet('CSS', bundleName)` appends `/index.css` to the entry or
bundle URL's path. Other section names use `/<encoded-name>/index.css`.
Query and fragment suffixes are preserved after the appended path. For example:

- `__LoadStyleSheet('CSS', '__Card__')`, with entry `https://app.test/main.js`,
  requests `https://app.test/main.js/index.css`.
- `__LoadStyleSheet('CSS', 'https://cdn.test/component.bundle?rev=2')`
  requests `https://cdn.test/component.bundle/index.css?rev=2`.

The wrapper constructs the specifier; URL resolution and transport policy
remain with the embedder. It can answer registered URLs, load CSS from a
server, or decode a container and supply preparsed styles through the same
completion. The stylesheet API adds no resource protocol or Rust bundle-name
lookup. A complete lazy-component script loader remains separate work.

## Load and adoption timing

Loading immediately returns a fresh opaque handle and queues a normal
stylesheet request. It does not mount styles or wait for IO. Resource absence
therefore cannot be reported by a synchronous null return: failed loads produce
one host `ScriptReported` error, including the requested URL. Boot readiness
continues to follow entry completion and the existing BTS readiness declaration.

`__AdoptStyleSheet(handle)` returns null and records an adoption. Ready
adoptions are applied in call order. If A is adopted before B, a faster B
response waits for A; a failed A is skipped so B can proceed. Repeated adoption
appends again and preserves the author cascade, specificity and importance.
Loading a sheet that is never adopted changes no styles.

Every load is a task of its view, using its existing cancellation token and
`SourceCompletion`. Completion re-enters through `Page::enter`, applies ready
adoptions and uses the normal commit/publication epilogue. Releasing a view
cancels unfinished loads and discards late results.

The JS WeakMap and finalizer retain only handle identity. A queued adoption
retains its resource even if its JS handle is collected before loading finishes.
Once mounted, styles belong to the document and survive handle collection.

## Source ownership and validation

`PageSource` registers named compiler CSS with `Resources::register_style_sheet`
using the same URLs as the JS wrapper. Native and web bundles share the
supported descriptor decoder. It lowers StyleRule, KeyframesRule and
FontFaceRule, compiler variables, fallbacks and trailing importance without
reconstructing a whole stylesheet. Malformed or unsupported descriptors are
not registered. Ordinary page styles keep their existing startup resource path;
native/web wire formats and the rkyv 0.7 model are unchanged.

Tests cover card aliases and explicit/escaped URLs, text/preparsed equivalence,
inert loads, out-of-order completion with ordered repeated adoption, CSS
precedence, collection before and after completion, load failure and view
cancellation. Native/web source integration verifies the final painted result
using only URLs and the embedder's resource loader.
