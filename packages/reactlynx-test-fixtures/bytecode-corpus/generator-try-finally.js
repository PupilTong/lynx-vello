// yield inside try/finally; the finally runs on normal completion.
var log = [];
function* g() {
  try { yield 'a'; log.push('after a'); }
  finally { log.push('finally'); yield 'from finally'; }
  yield 'end';
}
globalThis.__result = JSON.stringify([[...g()], log]);
