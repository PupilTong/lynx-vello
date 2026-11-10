// try/finally around await, with return and throw.
var log = [];
async function a() { try { return await 'value'; } finally { log.push('a finally'); } }
async function b() { try { await Promise.reject('x'); } finally { log.push('b finally'); await null; log.push('b after await in finally'); } }
async function main() {
  var r = await a();
  var r2;
  try { await b(); } catch (e) { r2 = 'rejected ' + e; }
  return [r, r2, log];
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
