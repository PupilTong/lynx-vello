// Computed keys in object literals, evaluated in order.
var log = [];
function k(v) { log.push(v); return v; }
var i = 0;
var o = { [k('a')]: 1, [k('b') + k('c')]: 2, ['n' + ++i]: 3, ['n' + ++i]: 4, [k(1) + 1]: 5 };
globalThis.__result = JSON.stringify([o, log]);
