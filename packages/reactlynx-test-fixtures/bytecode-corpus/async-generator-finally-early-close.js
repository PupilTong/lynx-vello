// Async generators with try/finally around yield and await, closed early by
// return(), throw() and break in for await; a finally block that awaits and yields.
var log = [];
async function* source() {
  try {
    try {
      yield 1;
      await null;
      yield 2;
    } finally {
      log.push('inner finally');
      await Promise.resolve();
      log.push('inner finally after await');
    }
    yield 3;
  } finally { log.push('outer finally'); }
}
async function* yieldsInFinally() {
  try { yield 'a'; } finally { yield 'from finally'; log.push('after finally yield'); }
}
async function main() {
  var out = [];
  var g1 = source();
  out.push(await g1.next(), await g1.return('early'), await g1.next());
  var g2 = source();
  await g2.next();
  try { await g2.throw(new TypeError('t')); } catch (e) { out.push('rethrown ' + e.name); }
  for await (const v of source()) { if (v === 2) break; out.push('saw ' + v); }
  var g3 = yieldsInFinally();
  out.push(await g3.next(), await g3.return('r'), await g3.next());
  var g4 = source();
  out.push(await g4.return('before start'), await g4.next());
  return [out, log];
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
