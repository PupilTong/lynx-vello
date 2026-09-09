// @ts-check
import "bobcat:worker";
import { createCrossThreadContext } from "bobcat:cross-thread-context";

// A Worker gets Lynx bindings by evaluating or importing this entry module.
// The bobcat:bts bootstrap appends await import(entryURL) after initializing
// the Context. Application module loading belongs to ResourceFetcher and is
// pending; the bootstrap carries no application source.
/** @type {any} */
const scope = globalThis;
const coreContext = createCrossThreadContext();

coreContext.connect((event) => scope.postMessage(event));
scope.addEventListener("message", (/** @type {{data: any}} */ event) => {
  coreContext.receive(event.data);
});

// This is the raw BTS environment's MVP. Loading a compiled ReactLynx BTS
// bundle also needs Lynx Core's module/init shell, which is not installed here.
export const lynx = {
  getCoreContext() {
    return coreContext;
  },
};
scope.lynx = lynx;
