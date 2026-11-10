// Own property order: integer keys ascending, then strings, then symbols, in insertion order.
var o = {};
o.b = 1; o[2] = 1; o.a = 1; o[1] = 1; o['10'] = 1; o[Symbol.iterator] = 1; o['-1'] = 1; o['01'] = 1;
var names = Reflect.ownKeys(o).map(function (k) { return typeof k === 'symbol' ? 'sym' : k; });
var forIn = [];
for (var k in o) forIn.push(k);
globalThis.__result = JSON.stringify([names, forIn]);
