import { reportException } from "bobcat:event-target";
import { invokeNativeModule } from "bobcat-internal:native-modules";

// The `bobcat:native-modules` ESM: the native module transport, for every
// realm kind.
//
// The BTS runtime builds the `NativeModules` object over it; this is the wire
// under that object. It imports nothing realm-specific — only the host member
// every realm kind declares and `bobcat:event-target`'s reporter — so it
// links in the MTS realm, the BTS and a plain `Worker` alike, and a realm
// reaches the embedder's modules through whichever table it was handed.
//
// Both halves travel as text, because both are JavaScript's own: the
// arguments are `JSON.stringify`d here and parsed by the embedder's module if
// it cares, and an answer comes back as JSON text this realm parses — the
// engine's rule that a payload only JavaScript owns stays opaque to Rust
// rather than acquiring a Rust model on the way through.
//
// A function argument cannot be stringified, so it is replaced with `null` and
// its index sent alongside; the host mints one `ModuleCallback` per index. Each
// is single-shot, as native's is (Android's `CallbackImpl.mInvoked`, iOS's
// `wrapperWasCalled`): the slot is cleared before the function runs, so a
// second answer for it — or a release arriving after an invocation — finds
// nothing and does nothing.

/**
 * The function arguments of every call still outstanding, by call id, sparse
 * by argument index. A call with no function arguments has no entry at all,
 * and an entry is deleted once its last function has been taken.
 */
const moduleCalls = new Map<number, (Function | undefined)[]>();
let nextModuleCall = 1;

/**
 * `NativeModules.<module>.<method>(...args)`: hands the call to the host and
 * returns `undefined`, because a module answers through its callbacks and
 * never through this call's result.
 */
export function callNativeModule(
  module: string,
  method: string,
  args: unknown[],
): undefined {
  const functions: (Function | undefined)[] = [];
  const indices: number[] = [];
  for (let index = 0; index < args.length; index++) {
    if (typeof args[index] === "function") {
      functions[index] = args[index] as Function;
      indices.push(index);
    }
  }
  // Serialized before anything is registered: `JSON.stringify` throws on a
  // BigInt or a cycle, and the throw belongs to the caller — a call that
  // never reached the host must leave no functions behind for an answer that
  // can never come.
  const encoded = JSON.stringify(
    args.map(argument => typeof argument === "function" ? null : argument),
  );
  const call = nextModuleCall++;
  if (indices.length) moduleCalls.set(call, functions);
  invokeNativeModule(call, module, method, encoded, indices.join(","));
  return undefined;
}

/**
 * One native module's answer, delivered by the host on this realm's own turn.
 *
 * `argumentsJson` is the JSON array to spread; `undefined` releases the
 * function without calling it, which is what a module that dropped its
 * callback owes. Either way the slot is cleared first, so the single-shot rule
 * holds even if the function calls back into the same module.
 */
export function __BobcatNativeModuleCallback(
  call: number,
  index: number,
  argumentsJson: string | undefined,
): undefined {
  const functions = moduleCalls.get(call);
  const callback = functions?.[index];
  if (functions) {
    functions[index] = undefined;
    if (!functions.some(entry => entry !== undefined)) moduleCalls.delete(call);
  }
  if (callback === undefined || argumentsJson === undefined) return undefined;
  try {
    callback(...JSON.parse(argumentsJson) as unknown[]);
  } catch (error) {
    // A module callback is application code like a listener, so it is
    // reported the way the realm reports an uncaught exception, and the
    // realm goes on.
    reportException(error);
  }
  return undefined;
}
