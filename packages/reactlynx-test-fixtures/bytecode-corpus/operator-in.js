// The in operator on own, inherited, array index and symbol keys.
var proto = { p: 1 };
var o = Object.create(proto);
o.own = undefined;
var arr = ['a', , 'c'];
var s = Symbol();
o[s] = 1;
var err;
try { 'x' in 'string'; } catch (e) { err = e.name; }
globalThis.__result = JSON.stringify(['own' in o, 'p' in o, 'q' in o, 0 in arr, 1 in arr, '2' in arr, 'length' in arr, s in o, 'toString' in o, err]);
