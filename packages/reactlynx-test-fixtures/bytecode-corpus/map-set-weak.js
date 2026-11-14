// Map, Set, WeakMap and WeakSet basics, with object keys and SameValueZero.
var k = {};
var m = new Map();
m.set(k, 'obj').set(NaN, 'nan').set(0, 'zero');
var s = new Set([1, 1, '1', NaN, NaN]);
var wm = new WeakMap([[k, 'weak']]);
var ws = new WeakSet([k]);
var err;
try { wm.set(1, 2); } catch (e) { err = e.name; }
globalThis.__result = JSON.stringify([m.get(k), m.get(NaN), m.get(-0), m.size, s.size, wm.get(k), ws.has(k), ws.has({}), err, [...m.keys()].length]);
