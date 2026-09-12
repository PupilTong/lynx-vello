# MTS same-thread execution

This layer is extracted from `codex/reactlynx-bts-mvp`, after the Worker resource
transport PR. It supplies Script evaluation, global lexical bindings, local Lepus
chunks and synchronous Promise-job checkpoints. Bundle lookup/cache,
`lynx.loadScript`'s public lookup wrapper, data/update/reload policy and compiled
BTS bootstrap remain separate layers.

## Native contract and ownership

The source audit used Lynx revision
`66b002855a25a5a8812fe878af69e20a346d0408`; `LoadScript`, `EvalBuf`,
`EvalLepusPendingTask`, `InternalCall` and `GetAndCall` were also checked against
Lynx 4.1.0. Relevant native owners are:

- `core/runtime/lepusng/quick_context.cc`: `InternalCall` and `EvalBuf`.
- `core/runtime/lepus/bindings/event/lepus_event_listener.cc`:
  `LepusClosureEventListener::Invoke`.
- `core/runtime/lepus/bindings/renderer_functions.cc`: `LoadScript` and
  `__LoadLepusChunk`; `core/renderer/template_entry.cc`: chunk lookup/evaluation.

| Entry | Successful return | Synchronous throw | Failed job |
| --- | --- | --- | --- |
| MTS function / engine listener | Drain jobs, then return the original value | Report; return undefined; skip nested drain | Report; stop drain; discard result |
| Global Script | Drain jobs, then return the original completion | Log; return null; skip nested drain | Report; continue drain; retain completion |
| Local Lepus chunk | Return true when found, after entry-scope evaluation | Report; still return true when found | Enclosing checkpoint owns jobs |

Unhandled Promise rejections report without replacing a successful function or
Script result. Neither execution entry awaits a returned Promise. A throwing
call leaves its queued jobs to the enclosing checkpoint. A render hook error
is nonfatal; an entry-module error still fails startup.

`callMts` uses the global object as receiver. Native engine dispatch wraps each
listener separately, so one listener's jobs finish before the next listener;
a failed listener does not stop the walk. Ordinary `EventTarget.dispatchEvent`
retains its existing JavaScript semantics. Boot uses this policy for its existing
processor, render hook and fallback event. This extraction preserves boot's
existing hook selection and payload shape; later lifecycle policy is separate.

Named cross-thread calls continue to await through the existing Worker
`postMessage` RPC implementation. Rust never resolves named application hooks or
schedules their replies.

## Script values and bindings

The bridge's private native `evaluateScript(source, filename)` invokes QuickJS
in global Script mode. It returns the JS value directly inside the realm:
objects, functions, thrown values and Promises retain identity, with no Rust
`HostValue` conversion. Script strict directives are honored independently of
the importing module. `var` and function declarations persist as global-object
properties; `let` and `const` persist in that realm's global lexical environment.

Boot installs its named runtime and Element PAPI values into the same lexical
environment once. It creates no `globalThis.lynx`, `console` or PAPI properties,
and requires no vendor global-binding lookup API. The private
`__BobcatEvaluateScript` wrapper owns error handling and the Script checkpoint;
its callers supply source text and a filename. Source retrieval remains outside
this helper. The installed input setter is retained for the later live-input
lifecycle layer; this PR supplies only the existing initial snapshots.

`PageSource` registers non-entry Lepus source chunks with an evaluator that
retains the selected entry's lexical scope. `__LoadLepusChunk` loads only a
matching local card chunk, repeats evaluation on every call, and returns false
for an absent chunk or a different component entry. This preserves the existing
ESM/direct-eval chunk boundary; it does not replace it with global Script mode.

## Checkpoints and lifetime

The bridge exposes a weak, owner-thread `JobQueue` handle and a separate `Fn`
reentrant host callback registration. The callback cannot retain its own realm
through that handle. Existing `FnMut` callbacks still refuse reentry. Core owns
the finite job budget, error reporting and checkpoint-generation notification;
the enclosing execution deadline still applies. Nested drains execute only JS
jobs, not timers, resource work, renderer commits or a nested executor.

The queue is runtime-wide, so sibling-realm jobs may run. Unhandled rejections
remain attributed to their own realm, and the generation notification lets a
sibling settle work completed by another realm's checkpoint. Arbitrary OOM job
failures are not covered by a native fixture oracle.

## Validation coverage

Real QuickJS regressions cover persistent globals, per-realm lexical isolation,
strict mode, TDZ, thrown-value identity and source locations; nested jobs,
finite work, rejection isolation and weak callback lifetime. Core tests cover
runtime/PAPI identity, original Script/Promise completions, repeat evaluation,
nonfatal reports, error logs/null results, processor-to-render ordering and
per-listener checkpoints. A source-native container test exercises selected
entry/chunk registration through the shipped resource host. JavaScript tests
cover missing/local chunk lookup and repeated failed evaluation.
