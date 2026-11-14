// An async generator yields promises of iterator results in order.
async function* g() { yield 1; var x = yield await Promise.resolve(2); yield 'got ' + x; return 'end'; }
var it = g();
var results = [];
it.next()
  .then(function (r) { results.push(r); return it.next(); })
  .then(function (r) { results.push(r); return it.next('sent'); })
  .then(function (r) { results.push(r); return it.next(); })
  .then(function (r) { results.push(r); return it.next(); })
  .then(function (r) { results.push(r); globalThis.__result = JSON.stringify([results, Object.prototype.toString.call(it)]); });
