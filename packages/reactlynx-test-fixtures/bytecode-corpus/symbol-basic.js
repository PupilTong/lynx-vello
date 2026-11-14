// Symbols as unique property keys, descriptions and the global registry.
var a = Symbol('desc'), b = Symbol('desc');
var o = { [a]: 1, plain: 2 };
var err;
try { a + ''; } catch (e) { err = e.name; }
globalThis.__result = JSON.stringify([a === b, a.description, String(a), o[a], Object.keys(o), Object.getOwnPropertySymbols(o).length, Symbol.for('k') === Symbol.for('k'), Symbol.keyFor(Symbol.for('k')), typeof Symbol.keyFor(a), err, JSON.stringify(o)]);
