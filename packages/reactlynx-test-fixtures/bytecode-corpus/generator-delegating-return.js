// return() on a generator delegating with yield* calls the inner return and both finally blocks.
var log = [];
function* inner() { try { yield 1; yield 2; } finally { log.push('inner finally'); } }
function* outer() { try { yield* inner(); } finally { log.push('outer finally'); } }
var it = outer();
it.next();
var r = it.return('stop');
var iterRet = { [Symbol.iterator]() { return { next() { return { value: 'x', done: false }; }, return(v) { log.push('iter return ' + v); return { value: 'iv', done: true }; } }; } };
function* viaIter() { yield* iterRet; }
var it2 = viaIter();
it2.next();
var r2 = it2.return('q');
globalThis.__result = JSON.stringify([r, r2, log]);
