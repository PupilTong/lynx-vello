// Class bodies are strict code.
class S {
  assign() { try { undeclaredInClass = 1; return 'no error'; } catch (e) { return e.name; } }
  thisValue() { return (function () { return this; })(); }
}
var s = new S();
globalThis.__result = JSON.stringify([s.assign(), s.thisValue() === undefined]);
