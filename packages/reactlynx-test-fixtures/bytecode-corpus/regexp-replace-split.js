// replace with a function and special patterns, split with captures, matchAll.
var r1 = 'a1b22c333'.replace(/\d+/g, function (m, off) { return '[' + m.length + '@' + off + ']'; });
var r2 = 'john smith'.replace(/(\w+)\s(\w+)/, '$2, $1 ($&) $$');
var r3 = 'a, b,c'.split(/\s*,\s*/);
var r4 = 'a1b2'.split(/(\d)/);
var r5 = Array.from('x1y22'.matchAll(/\d+/g), m => m[0] + m.index);
var r6 = 'aaa'.replaceAll('a', 'b');
globalThis.__result = JSON.stringify([r1, r2, r3, r4, r5, r6]);
