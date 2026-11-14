// for-of over a generator object.
function* range(a, b) { for (let i = a; i < b; i++) yield i; }
var out = [];
for (const v of range(0, 4)) out.push(v);
var g = range(10, 13);
for (const v of g) { out.push(v); break; }
out.push(JSON.stringify(g.next()));
globalThis.__result = JSON.stringify(out);
