// A null link short-circuits the rest of the chain, including side effects in it.
var count = 0;
function bump() { count++; return 'k'; }
var n = null;
var r1 = n?.[bump()].x.y;
var r2 = n?.a.b(bump());
var o = { a: null };
var r3 = o.a?.[bump()];
var r4 = delete n?.x;
var r5 = delete o?.a;
globalThis.__result = JSON.stringify([String(r1), String(r2), String(r3), count, r4, r5, 'a' in o]);
