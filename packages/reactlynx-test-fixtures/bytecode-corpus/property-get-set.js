// Property reads and writes with dot and bracket syntax.
var o = { a: 1 };
o.b = 2;
o['c'] = 3;
var k = 'd';
o[k] = 4;
o[5] = 'five';
o[1.5] = 'frac';
var missing = o.zzz;
var arr = [];
arr[2] = 'x';
globalThis.__result = JSON.stringify([o, typeof missing, o['5'], o['1.5'], arr.length, arr]);
