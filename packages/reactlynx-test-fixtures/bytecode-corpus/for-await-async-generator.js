// for await over an async generator, with break calling return.
var log = [];
async function* g() { try { for (let i = 0; i < 5; i++) { await null; yield i; } } finally { log.push('gen finally'); } }
async function main() {
  var out = [];
  for await (const v of g()) { out.push(v); if (v === 2) break; }
  return [out, log];
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
