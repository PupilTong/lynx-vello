// A thrown error's stack, for the converted debug information.
function thrower() {
  throw new Error('here');
}
try {
  thrower();
} catch (e) {
  globalThis.__result = e.stack;
}
