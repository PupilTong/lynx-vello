// return() on a suspended generator finishes it and runs finally.
var log = [];
function* g() { try { yield 1; yield 2; } finally { log.push('cleanup'); } }
var it = g();
var out = [it.next(), it.return('early'), it.next()];
var fresh = g();
out.push(fresh.return('before start'), fresh.next());
function* h() { try { yield 1; } finally { yield 'from finally'; } }
var it2 = h();
it2.next();
out.push(it2.return('r'), it2.next());
globalThis.__result = JSON.stringify([out, log]);
