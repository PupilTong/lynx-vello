# Data and global-property lifecycle

This layer supplies the runtime side of initial data, processors, update/reset,
global properties and reload. It runs over the existing MTS realm and BTS Worker;
framework module loading and lazy component execution have separate owners.
Raw ESM/XML entries can install the lifecycle hooks directly. Compiled ReactLynx
fixtures remain with the module/bootstrap integration until that layer is split.

## Initial state and processor results

Boot parses the host's `init_data` and `global_props` JSON in MTS and initializes
its environment before importing the entry. It retains the initial data argument
independently of `lynx.__initData`: React initialization can replace that slot.
After the entry finishes, `processData(data, processorName)` runs through the
ordinary JS call boundary. The returned value is used synchronously; Promise
jobs do not run between lifecycle hooks or before the BTS snapshot. Boot queues
its flush with `Promise.resolve().then` after rendering.
Only a non-null, non-array object replaces the input. A non-table result or a
reported processor failure preserves the original data.

Before rendering, boot posts the processed result, host props and SystemInfo as
its first `worker.postMessage`. Worker transport supplies the ordinary JSON copy;
there is no native bootstrap-data binding, JSON map or generated data-bearing
BTS module. The BTS bootstrap installs its receiver and returns, allowing the
initialization message to arrive. JS initializes its inputs, imports the entry,
and posts `backgroundReady` on success or `backgroundFailed` on failure.
Context/lifecycle messages received during that import wait on its Promise. Undefined object members are omitted, nonfinite numbers become null,
and negative zero becomes zero; own `__proto__` keys remain ordinary data. Rust
carries JSON protocol data, not realm values or DOM handles. BTS receives the
parsed data before its entry runs:
`_params.initData` is null, `_params.updateData` and `lynx.__initData` share the
processed data, and `_params.cacheData` is empty under the default host policy.
The runtime reads `PageConfig.enable_js_data_processor` and `Viewport` from the
staged document ingredients. `ViewSources.initial_processor` remains a plain
`String`: the existing one-shot startup-data binding hands it directly to JS,
without serializing it or embedding it in generated source. JS constructs
SystemInfo from its runtime constants and viewport metrics, then sends it to BTS.
Initial global props come solely from `ViewSources.global_props`.

`ViewSources.initial_processor` selects the initial name. Host update/reset/reload
accept the JSON data as a `String` and the processor name as a separate `String`;
an empty name selects the default processor. Global-property updates likewise
accept a JSON object string; global events accept their argument list as a JSON
array string. The embedder owns serialization. Core carries these strings
unchanged through the command channel and passes them as JS call arguments;
it neither parses the data nor serializes a message envelope. MTS uses
`JSON.parse` before invoking hooks and constructs the Worker message in JS. The framework's `processData` owns named lookup and
fallback. After MTS processing, BTS receives an empty processor name, preventing
a second preprocessing pass. The `PageConfig.enable_js_data_processor` switch comes
from the normalized `enableJSDataProcessor` source flag: if enabled, the runtime
passes raw data/name through rather than inventing a BTS processor. The default
is false. No pre-update cache, processor coalescing or path-based merge is added.

## Readiness and delivery order

MTS render/flush completes without waiting for BTS. Public readiness requires
both MTS completion and the BTS acknowledgement. `LynxView::pump` records it
before returning `ScriptFinished`; `is_ready()` exposes the same state.

Every host lifecycle operation (`update_data`, `reset_data`, `update_global_props`,
`reload`, and `send_global_event`) returns `EngineError::NotReady` until readiness
has been observed or after the view ends. Rejected commands never enter the
channel. Embedders supply initial data/props in `ViewSources`, then wait for
readiness before sending updates. There is no pre-realm props cache, early-update
policy, initial-render flag or replay queue for host updates.

Accepted updates use the existing ordered command and Worker channels. Internal
MTS messages produced by entry evaluation or rendering still retain their order
before Worker connection and while the BTS entry imports. React owns data merging,
RESET semantics, rerendering and component state; Rust sends a command and
JavaScript invokes the framework's current hook.

Accepting a command does not validate its JSON. Malformed data fails when MTS
parses it, before any lifecycle hook or Worker message, and follows the existing
script-failure path: `pump` reports `ScriptRunError` and the view ends.

Global props merge literal top-level keys; nested objects are replaced and dots
in a key stay literal. MTS keeps the host-provided values as JSON text independently
of the mutable objects exposed to application code.
Each later MTS update replaces the exported `__globalProps`, `lynx.__globalProps`
so existing ESM imports observe the new value. BTS receives its own snapshot.

Engine events (`__RenderPage`, `__UpdatePage`, `__RemoveComponents`,
`__UpdateGlobalProps`) take precedence over legacy global functions and carry an
argument array through the engine EventTarget. Listeners retain the engine as
their receiver, and no origin field is added. Hooks run synchronously; Promise
jobs run at the existing outer checkpoint. A hook failure reports without preventing the remaining
lifecycle steps or an already-required BTS notification.

## Reload

`LynxView::reload` retains the realms and entry module. It processes data, removes
components, queues `onAppReload`, then calls the MTS update with
`reloadTemplate:true`. The app reload must precede the new first-screen lifecycle
event in BTS. The framework owns cleanup, state recreation and rehydration.

`lynx.reload(value?, callback?)` sends a Worker message. Omitted/null/primitive
values become an empty object; top-level functions or arrays do not reload.
Objects are serialized by Worker.postMessage at the call; ordinary JSON value
limits apply. Non-function callbacks are ignored.
This path skips the MTS processor and sets `reloadFromJS:true`. Its dedicated
acknowledgement is queued with `Promise.resolve().then` after MTS lifecycle calls.
Already queued jobs precede it; nested jobs may follow.
The BTS callback receives zero arguments and undefined `this`, once only, even
when it throws. It does not wait for a new hydration commit or use named RPC's
await-result boundary. The callback map remains owned by its realm.

## Evidence and tests

Native evidence at `66b002855a25a5a8812fe878af69e20a346d0408`:

- `core/runtime/js/js_app.cc:2031–2078,2117–2135`: encoded versus host data slots.
- `core/renderer/template_assembler.cc:532–560,2225–2233,2288–2370,3342–3404`:
  processor fallback, default early-update policy and propagation.
- `template_assembler.cc:250–290,433–435,2930–2951`,
  `core/shell/lynx_shell.cc:1525–1534` and `core/shell/tasm_mediator.cc:855–876`:
  global-property storage, delivery and snapshots.
- `core/runtime/lepusng/quick_context.cc:1200–1239`: call failures return to the
  assembler. The intentionally ordinary JS call/microtask boundary is described in
  [MTS execution](mts-execution-runtime.md).

Core tests cover readiness, initial argument retention, processor names/fallbacks,
cross-realm snapshots, live ESM props, failed BTS entry loading and Promise-job
order. JS tests cover engine/global-hook precedence, merged props, update/reset
options, both reload paths, coercion and callback release. The source integration
rejects public updates before readiness, including while BTS loads; after readiness
it verifies FIFO delivery, unchanged entry execution count and a BTS-origin reload.
It also checks SystemInfo and initial data in both entries.
