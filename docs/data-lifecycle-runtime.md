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
same-thread call boundary. Its Promise jobs finish before the result is used.
Only a non-null, non-array object replaces the input. A non-table result or a
reported processor failure preserves the original data.

Before rendering, boot snapshots the processed result and current host props
with ordinary JSON serialization and supplies the data to the BTS
Worker. Undefined object members are omitted, nonfinite numbers become null,
and negative zero becomes zero; own `__proto__` keys remain ordinary data. Rust
carries JSON protocol data, not realm values or DOM handles. BTS receives the
parsed data before its entry runs:
`_params.initData` is null, `_params.updateData` and `lynx.__initData` share the
processed data, and `_params.cacheData` is empty under the default host policy.
The same initial environment supplies SystemInfo and global props to both realms.

`DataProcessing.initial_processor` selects the initial name; a plain JSON map
passed to a host update selects the default name. `DataUpdate` carries an explicit
name for update/reset/reload. The framework's `processData` owns named lookup and
fallback. After MTS processing, BTS receives an empty processor name, preventing
a second preprocessing pass. The retained `DataProcessing.on_js` switch comes
from the normalized `enableJSDataProcessor` source flag: if enabled, the runtime
passes raw data/name through rather than inventing a BTS processor. The default
is false. No pre-update cache, processor coalescing or path-based merge is added.

## Readiness and delivery order

The initial MTS render and BTS readiness are different moments. Boot marks the
former after render and flush. MTS evaluation then completes without awaiting
BTS. A BTS ready message makes MTS call the native `notifyReady()` binding;
public `is_ready()` becomes true when the host observes `ScriptFinished`.

| Host operation | Before initial MTS render | After initial MTS render |
| --- | --- | --- |
| update/reset | Ignore, without later replay | Process, invoke MTS update, then enqueue BTS update |
| global props | Retain merged host props; update the initial environment without hooks | Enqueue full props to BTS, then update MTS bindings and invoke its hook |
| reload | Report nonfatally and discard | Process, remove components, enqueue BTS reload, then invoke MTS update |
| global event | Existing observed-readiness requirement | Still requires observed `ScriptFinished` |

The ordered Worker consumer retains these messages while its entry loads.
The public update methods do not add a second readiness queue. React owns data
merging, RESET semantics, rerendering and component state; Rust sends a command
and JavaScript invokes the framework's current hook.

Global props merge literal top-level keys; nested objects are replaced and dots
in a key stay literal. The host retains JSON text independently of mutable script objects.
Each later MTS update replaces the exported `__globalProps`, `lynx.__globalProps`
and the existing global Script lexical binding. BTS receives its own snapshot.

Engine events (`__RenderPage`, `__UpdatePage`, `__RemoveComponents`,
`__UpdateGlobalProps`) take precedence over legacy global functions and carry an
argument array with `Engine` origin. Each listener/call uses the existing MTS
Promise-job checkpoint. A hook failure reports without preventing the remaining
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
acknowledgement is sent after the MTS lifecycle calls and their Promise jobs;
the BTS callback receives zero arguments and undefined `this`, once only, even
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
  assembler. The same-thread checkpoint contract lives in
  [MTS execution](mts-execution-runtime.md).

Core tests cover pre-realm and pending-import readiness, initial argument
retention, processor names/fallbacks, cross-realm snapshots, Script lexical props
and Promise-job order. JS tests cover engine/global-hook precedence, merged props,
update/reset options, both reload paths, coercion and callback release. The source
integration drives public `LynxView` methods while holding the BTS entry, then
verifies FIFO delivery, unchanged entry execution count and a BTS-origin reload.
