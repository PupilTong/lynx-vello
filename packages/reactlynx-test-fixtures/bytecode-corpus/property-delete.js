// delete on own, inherited, non-configurable and array properties.
var o = { a: 1, b: 2 };
var r1 = delete o.a;
var r2 = delete o.zzz;
var child = Object.create({ inherited: 1 });
var r3 = delete child.inherited;
var frozen = Object.freeze({ x: 1 });
var r4 = delete frozen.x;
var arr = [1, 2, 3];
var r5 = delete arr[1];
var r6 = (function () { 'use strict'; try { delete frozen.x; return 'no error'; } catch (e) { return e.name; } })();
globalThis.__result = JSON.stringify([r1, r2, r3, child.inherited, r4, frozen.x, r5, arr.length, 1 in arr, r6, Object.keys(o)]);
