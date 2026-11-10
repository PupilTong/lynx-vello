// Compound assignment on members, with getters and setters observed.
var log = [];
var o = { a: 1, s: 'x', get acc() { log.push('get'); return 5; }, set acc(v) { log.push('set ' + v); } };
o.a += 2;
o.a *= 10;
o.s += 'y';
o.acc += 1;
o.acc **= 2;
o.missing += 1;
var nested = { inner: { v: 1 } };
nested.inner.v <<= 3;
globalThis.__result = JSON.stringify([o.a, o.s, String(o.missing), nested.inner.v, log]);
