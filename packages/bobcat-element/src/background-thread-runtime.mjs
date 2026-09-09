// @ts-check
import "bobcat:worker";
import { createCrossThreadContext } from "bobcat:cross-thread-context";

// A Worker gets Lynx bindings by evaluating or importing this entry module.
// The native worker queue starts delivery after the entry finishes, so the
// entry can register typed listeners before MTS's queued events arrive.
/** @type {any} */
const scope = globalThis;
const coreBridge = createCrossThreadContext();

coreBridge.connect((event) => scope.postMessage(event));
scope.addEventListener("message", (/** @type {{data: any}} */ event) => {
  coreBridge.receive(event.data);
});

// This is the raw BTS environment's MVP. Loading a compiled ReactLynx BTS
// bundle also needs Lynx Core's module/init shell, which is not installed here.
export const lynx = {
  getCoreContext() {
    return coreBridge.context;
  },
};
scope.lynx = lynx;
