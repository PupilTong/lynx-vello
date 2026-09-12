# Element PAPI and BTS node queries

This layer follows the native SelectorQuery facade used by ReactLynx. It runs
on the existing raw BTS environment, without requiring `lynx_core.js` or the
pending compiled-bundle initialization shell.

`bobcat:selector-query` owns `SelectorQuery` and `NodesRef` on BTS.
`lynx.createSelectorQuery()` sends operation, selection token, parameters and an
optional callback ID through `globalThis.postMessage`. MTS receives the request
through its existing Worker, calls `__BobcatQueryNodes` in `bobcat:element`, and
replies through `worker.postMessage`. The existing Worker JSON transport copies
messages at send time. Rust transports Worker messages unchanged; only MTS host
members access the document. Named Lepus RPC keeps the asynchronous boundary
established in the preceding stack layer.

## Selection and callbacks

The native token contains `type`, `identifier`, `component_id`, `first_only` and
`root_unique_id`. Appending a task returns a new query; `exec()` replays the
queue without clearing it. Selection captures its root at select time.

- CSS selection uses the document's standard selector engine, including the
  Lynx query root. `selectRoot()` selects that root itself.
- `selectUniqueID()` is global once the supplied root exists.
- Legacy `selectReactRef()` selects by the `react-ref` attribute and runs the
  next task immediately; it is refused after queued work.
- `fields()` and `path()` call back with `(data, status)`. A successful status
  is `{code: 0, data: "success"}`. Missing nodes return code 2 with null for a
  single result or an empty array for multiple results; invalid selectors use
  code 5. Result JSON serialization failures reply with code 1 and release the callback.
- `invoke()` calls `success(result.data)` or `fail(result)`. Selecting all nodes
  fails locally with code 5. Missing targets use code 2; an existing target
  currently fails with code 1 because actual UI methods are unimplemented.
  A failure without a fail callback is ignored by the production facade.
- BTS owns callbacks and removes each after delivery, including callbacks that
  throw. Query callbacks run in the message handler; they are distinct from the
  Promise continuation used by named Lepus RPC.

## Values and mutations

Fields support `id`, `tag`, `unique_id`, `name`, `index`, `class`, `attribute`,
`dataset`/`dataSet`, and `query`. For `query: true`, BTS requests `unique_id` and
constructs a rooted query in the response callback. `path()` returns field
records from the selected node through its page ancestors.

`__SetDataset` merges typed keys; `__AddDataset` replaces one key. Assignment
and `__GetDataset` copy containers. Datasets stay separate from DOM `data-*`
strings. Background event descriptors include those typed dataset values.
Lynx attributes similarly retain typed copies for readback while ordinary DOM
attributes remain strings. Attribute fields exclude function/null/undefined
values and the separate id/class/style/dataset stores. Assigning a local
attribute does not impose the Worker transport's JSON restrictions.

`__AddInlineStyle` changes one named CSS property without replacing the rest
of the declaration block. Empty/nullish values remove it; the existing CSSOM
path owns parsing and invalid-value handling. `setNativeProps` classifies names
through Stylo, updates CSS properties or attributes, then commits before the
next queued query. Inputs use CSS strings; numeric native CSS property IDs and
native numeric-length conversion remain unsupported.

## Remaining boundaries

Legacy component-scoped roots, direct MTS selector PAPI, actual UI invoke
methods, animation methods, dataset-to-DOM reflection and cross-realm host
objects remain pending. The complete Lynx diagnostics layer is also pending:
query status replies work now, the MTS error reporter remains its existing sink,
and invalid legacy ReactRef chaining uses the Worker's existing error path.
Compiled ReactLynx bundle loading and ref hydration integration remain later
stack layers; these tests exercise raw BTS with real QuickJS and document nodes.

## Native evidence and verification

The reference checkout is `lynx/` as located in `AGENTS.md`:

- `js_libraries/lynx-core/src/modules/selectorQuery/SelectorQuery.ts`: task
  branching/replay, selection tokens, rooted queries and invoke callbacks.
- `core/renderer/dom/fiber/fiber_node_info.cc`: fields, typed datasets and
  attribute filtering, plus path-to-root order.
- `core/renderer/dom/attribute_holder.cc`: dataset merge behavior.
- `core/renderer/page_proxy.cc`: structural result/status shapes and selection.

Rstest checks the facade, container copies, JSON serialization failures and callback cleanup.
Core tests run both real QuickJS realms, query the actual DOM, and verify that
native props are applied before a later query observes them. Inline-style
coverage checks the resulting declaration block and layout through Stylo.
