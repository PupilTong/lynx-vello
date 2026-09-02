// Test-only adapter for the native `bobcat-internal:host` ESM. Rstest aliases
// that specifier here; production QuickJS supplies the same named exports
// directly from Rust.

const native = globalThis.__bobcatTestHost;
if (native === null || typeof native !== "object") {
  throw new Error("the Element PAPI test native host is not installed");
}

export const createPage = native.createPage;
export const createElement = native.createElement;
export const setAttribute = native.setAttribute;
export const setInlineStyles = native.setInlineStyles;
export const removeAttribute = native.removeAttribute;
export const getAttribute = native.getAttribute;
export const tagName = native.tagName;
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
