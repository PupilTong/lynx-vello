// The length and name of functions, methods, arrows and bound functions.
function decl(a, b) {}
var expr = function (a) {};
var arrow = (a, b, c) => {};
var o = { method(x) {}, ['comp' + 'uted']() {}, get acc() { return 1; } };
var bound = decl.bind(null, 1);
var out = [decl.length, decl.name, expr.name, arrow.name, arrow.length, o.method.name, o.computed.name, Object.getOwnPropertyDescriptor(o, 'acc').get.name, bound.name, bound.length, (function (a, b = 1, c) {}).length];
globalThis.__result = JSON.stringify(out);
