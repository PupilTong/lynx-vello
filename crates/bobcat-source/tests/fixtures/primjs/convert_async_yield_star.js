// An async generator delegates to an async iterator whose second next()
// rejects: the rejection propagates and the iterator's return() is not
// called (PrimJS's yield* has no iterator marker in its catch slot).
var log = [];
var inner = {
  n: 0,
  [Symbol.asyncIterator]() { return this; },
  next() { this.n++; return this.n === 1 ? Promise.resolve({ value: 1, done: false }) : Promise.reject(new Error('boom')); },
  return() { log.push('return called'); return Promise.resolve({ done: true }); },
};
async function* g() { yield* inner; }
(async function () {
  var it = g();
  log.push((await it.next()).value);
  try { await it.next(); } catch (e) { log.push(e.message); }
  globalThis.__result = JSON.stringify(log);
})();
