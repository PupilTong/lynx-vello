// Prefix and postfix ++/-- on members return old and new values.
var o = { n: 5, s: '3' };
var out = [o.n++, o.n, ++o.n, o.n--, --o.n, o.s++, o.s];
var counter = { get v() { return 7; }, set v(x) { out.push('set ' + x); } };
out.push(counter.v++, ++counter.v);
var u = {};
out.push(String(u.missing++), String(u.missing));
globalThis.__result = JSON.stringify(out);
