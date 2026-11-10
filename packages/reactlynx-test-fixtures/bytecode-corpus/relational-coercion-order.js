// ToPrimitive is called on operands left to right.
var log = [];
function mk(name, v) { return { valueOf: function () { log.push(name); return v; } }; }
var r1 = mk('a', 1) < mk('b', 2);
var r2 = mk('c', 3) > mk('d', 1);
var r3 = mk('e', 1) + mk('f', 2);
var r4 = mk('g', 5) - mk('h', 1);
globalThis.__result = JSON.stringify([r1, r2, r3, r4, log]);
