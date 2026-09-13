# Named stylesheet loading and adoption

Named CSS has a source-side decoder and an MTS-only handle table. The decoder
produces the same `PreparsedStyleSheet` vocabulary as ordinary page styles;
the runtime lowers it into document-branded Stylo rules. This layer supports
already-decoded default-page metadata under `__Card__`. External bundle fetching,
caching and component execution are separate work; no resource request is made
by loading or adopting a stylesheet.

## Source ownership

Native decoding and explicit native-to-web conversion already preserve named
`{encoding:"CSS", content:{ruleList}}` descriptors. `custom_style::named_style_sheets`
selects those descriptors and lowers StyleRule, KeyframesRule and FontFaceRule,
including compiler variable placeholders, fallbacks and token-level trailing
`!important`. It does not reconstruct a whole stylesheet. Font-face descriptors
retain their separate descriptor grammar. Native imports have already been
flattened into each named fragment in cascade order. The rkyv 0.7 layout and
binary section formats are unchanged.

Malformed or unsupported descriptors do not become partially loadable sheets.
MediaRule, SupportsRule, LayerRule and ImportRule descriptors are unsupported;
invalid CSS syntax inside a supported descriptor goes through Stylo's normal
rule/declaration handling. Real native bytecode and irreversible CSS encodings
remain source-parser errors.

`PageSource` retains the ordinary sheet and the decoded named sheets in an
`Arc<BundleSource>` and supplies it through `ViewSources::page_bundle`. Ordinary
StyleInfo is still mounted during document construction. The named map is only
metadata until explicitly adopted. Each view's outbox supplies this immutable
metadata to its runtime through `SourceRequester::bundle`; at this stage only
`__Card__` resolves. XML sources supply no bundle metadata.

## MTS handles and component styles

`__LoadStyleSheet(key, bundleName)` requires two strings. Missing bundles or
named sheets return `null`. Each successful load lowers fresh document-branded
rules, stores them under a private native id and returns a fresh frozen JS
object. Loading changes no cascade. A WeakMap associates each object with its
id; a FinalizationRegistry releases the native table entry after collection.

`__AdoptStyleSheet(handle)` accepts only a handle minted by that realm, appends
its rules to the document and returns `null`. Repeated adoption appends again
in call order. The document retains the adopted rules independently, so freeing
a handle cannot remove styles. Rules enter the existing author cascade:
specificity and `!important` still apply, and fragments are global under the
existing `enableRemoveCSSScope` policy. No component CSS-id scoping is added.

The private `adoptComponentStyleSheet(bundleName)` host entry appends a decoded
bundle's ordinary sheet through the same lowering/cascade path. A missing
bundle is an error; a bundle without an ordinary sheet is a no-op. This is the
component-style append operation; the later component loader will decide when
to call it after an external bundle becomes available. Tests use `__Card__` to
exercise the operation without adding that transport or execution layer.

The runtime owns all handles and rules on MTS. Neither a JS wrapper nor a DOM
handle crosses Worker messages or the embedder boundary.

## Native contract and validation

At native revision `66b002855a25a5a8812fe878af69e20a346d0408`, see
`core/runtime/lepus/bindings/renderer_functions.cc:250,916–1030`,
`base/include/value/base_value.h:116` and
`core/renderer/dom/element_manager.h:438–440`:
loading creates a fresh `SharedCSSFragmentWrapper` without mounting it;
adoption appends and dirties styles, permits repetition and retains the rules.
The `RETURN_UNDEFINED` macro actually returns a default null-tagged value,
which explains React's explicit `styleSheet !== null` check. Native compiler
compatibility gates precede fragment decoding and do not apply to core's
already-normalized style vocabulary. Standard CSS cascade semantics follow
the repository's W3C policy.

Source tests cover supported descriptor grammars, malformed data, native input
and explicit web conversion, with distinct named sections in separate views.
Runtime tests check inert loads, missing sheets/bundles, fresh handles, repeated
adoption, source order, specificity, importance, handle collection and component
style append. JS tests check opaque handle identity and argument validation.
