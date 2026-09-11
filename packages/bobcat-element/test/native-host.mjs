// Test-only adapter for the native `bobcat-internal:host` ESM. Rstest aliases
// that specifier here; production QuickJS supplies the same named exports
// directly from Rust.
//
// This file is the build-time resolution target of every `bobcat-internal:host`
// import in the suite, element-papi.mjs's included: rstest.config.ts points a
// `NormalModuleReplacementPlugin` at it. At run time element-papi.test.js's
// `rstest.mockRequire` shadows these exports with its own recording mock, so
// what the tests observe is that mock — but the module still has to exist and
// still has to carry every export the runtime imports, or resolution fails
// before any mock is consulted. It is kept in step with the native module by
// hand.

const native = globalThis.__bobcatTestHost;
if (native === null || typeof native !== "object") {
  throw new Error("the Element PAPI test native host is not installed");
}

export const createDocument = native.createDocument;
export const createPage = native.createPage;
export const createElement = native.createElement;
export const setAttribute = native.setAttribute;
export const setInlineStyles = native.setInlineStyles;
export const removeAttribute = native.removeAttribute;
export const getAttribute = native.getAttribute;
export const tagName = native.tagName;
export const attributeNames = native.attributeNames;
export const childElementIds = native.childElementIds;
export const parentNode = native.parentNode;
export const insertBefore = native.insertBefore;
export const removeElement = native.removeElement;
export const replaceElement = native.replaceElement;
export const swapElement = native.swapElement;
export const dropElement = native.dropElement;
export const flushElementTree = native.flushElementTree;
export const enableEventListener = native.enableEventListener;
export const disableEventListener = native.disableEventListener;
export const stopPropagation = native.stopPropagation;
export const setTimer = native.setTimer;
export const clearTimer = native.clearTimer;
export const initData = native.initData;
export const globalProps = native.globalProps;
