// Optional calls keep this and skip the call for null or undefined.
var o = { v: 'o', m() { return this.v; }, n: null };
var u;
var out = [o.m?.(), o.missing?.(), o.n?.(), u?.m(), o?.m(), o?.['m']?.(), (o?.m)()];
var err;
try { o.v?.(); } catch (e) { err = e.name; }
out.push(err);
globalThis.__result = JSON.stringify(out.map(String));
