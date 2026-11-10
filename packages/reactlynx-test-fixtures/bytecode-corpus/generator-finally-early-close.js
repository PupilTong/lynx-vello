// Generators with try/finally around yield, closed early by return(), throw(),
// break in for-of and destructuring; finally blocks that yield or return during the
// close.
var log = [];
function* plain() {
  try {
    try { yield 1; yield 2; } finally { log.push('inner finally'); }
    yield 3;
  } finally { log.push('outer finally'); }
}
function* yieldsInFinally() {
  try { yield 'a'; } finally { log.push('cleanup start'); yield 'from finally'; log.push('cleanup end'); }
}
function* returnsInFinally() {
  try { yield 'x'; } finally { return 'finally return'; }
}
function* catches() {
  while (true) {
    try { yield 'waiting'; } catch (e) { log.push('caught ' + e); } finally { log.push('loop finally'); }
  }
}
var out = [];
var g1 = plain(); out.push(g1.next(), g1.return('r'), g1.next());
var g2 = plain(); g2.next(); g2.next(); try { g2.throw('t'); } catch (e) { out.push('rethrown ' + e); }
for (var v of plain()) { if (v === 2) break; }
var [first] = plain();
out.push(first);
var g3 = yieldsInFinally(); out.push(g3.next(), g3.return('r3'), g3.next(), g3.next());
var g4 = returnsInFinally(); out.push(g4.next(), g4.return('ignored'));
var g5 = catches(); out.push(g5.next(), g5.throw('e1'), g5.throw('e2'), g5.return('done'));
globalThis.__result = JSON.stringify([out, log]);
