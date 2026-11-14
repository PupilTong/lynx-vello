// throw() on a delegating generator reaches the inner generator first.
var log = [];
function* inner() { try { yield 1; } catch (e) { log.push('inner caught ' + e); yield 'recovered'; } }
function* outer() { yield* inner(); yield 'after'; }
var it = outer();
var out = [it.next().value, it.throw('E').value, it.next().value];
var noThrow = { [Symbol.iterator]() { return { next() { return { value: 1, done: false }; }, return() { log.push('closed'); return {}; } }; } };
function* viaIter() { try { yield* noThrow; } catch (e) { log.push('outer caught ' + e.name); } }
var it2 = viaIter();
it2.next();
it2.throw('X');
globalThis.__result = JSON.stringify([out, log]);
