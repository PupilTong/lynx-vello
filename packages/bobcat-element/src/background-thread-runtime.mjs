// @ts-check
import "bobcat:worker";
import { createCrossThreadContext } from "bobcat:cross-thread-context";

// The bobcat:bts bootstrap and the BTS application's entry preamble import
// this runtime. Like MTS, lynx is a module binding, never a global property.
// Application module loading through ResourceFetcher remains pending.
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
