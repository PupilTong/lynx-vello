// for-of over Map and Set in insertion order, with mutation during iteration.
var m = new Map([['a', 1], ['b', 2]]);
m.set('c', 3);
var out = [];
for (const [k, v] of m) { out.push(k + v); if (k === 'a') m.delete('b'); }
for (const k of m.keys()) out.push(k);
for (const v of m.values()) out.push(v);
var s = new Set([3, 1, 3, 2]);
for (const v of s) { out.push('s' + v); if (v === 2) s.add(9); }
globalThis.__result = JSON.stringify(out);
