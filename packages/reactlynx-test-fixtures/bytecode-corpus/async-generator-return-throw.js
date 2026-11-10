// return() and throw() on a suspended async generator.
var log = [];
async function* g() { try { yield 1; yield 2; } catch (e) { log.push('caught ' + e); yield 'recovered'; } finally { log.push('finally'); } }
async function main() {
  var a = g();
  var r1 = await a.next();
  var r2 = await a.return('early');
  var b = g();
  await b.next();
  var r3 = await b.throw('boom');
  var r4 = await b.next();
  return [r1, r2, r3, r4, log];
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
