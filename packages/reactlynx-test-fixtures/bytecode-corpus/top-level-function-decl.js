// Top-level function declarations are hoisted and become global properties.
var early = hoistedFn(2);
function hoistedFn(x) { return x * 10; }
var desc = Object.getOwnPropertyDescriptor(globalThis, 'hoistedFn');
function redeclared() { return 'first'; }
function redeclared() { return 'second'; }
globalThis.__result = JSON.stringify([early, typeof globalThis.hoistedFn, desc.enumerable, desc.configurable, redeclared()]);
