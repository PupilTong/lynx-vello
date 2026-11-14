// Ordering of await continuations and promise reactions.
var log = [];
async function a() { log.push('a1'); await null; log.push('a2'); await null; log.push('a3'); }
async function b() { log.push('b1'); await Promise.resolve(); log.push('b2'); }
Promise.resolve().then(function () { log.push('p1'); }).then(function () { log.push('p2'); });
a();
b();
log.push('sync');
var done = Promise.resolve();
for (var i = 0; i < 6; i++) done = done.then(function () {});
done.then(function () { globalThis.__result = JSON.stringify(log); });
