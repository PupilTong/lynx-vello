// Defaults apply for undefined only and are evaluated lazily, left to right.
var log = [];
function d(v) { log.push(v); return v; }
var [a = d('a'), b = d('b'), c = a] = [undefined, null];
var { x = d('x'), y = d('y'), z: { w = d('w') } = {} } = { y: 0 };
var [p = 1, q = p + 1] = [];
globalThis.__result = JSON.stringify([a, b, c, x, y, w, p, q, log]);
