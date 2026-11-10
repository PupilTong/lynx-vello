// yield* delegating to an iterator that has only next: calling return() on the outer
// generator completes it, calling throw() closes nothing on the inner side and
// raises a TypeError.
var log = [];
function bare() {
  var i = 0;
  var it = {
    next(v) { log.push('next ' + v); i++; return i <= 3 ? { value: 'v' + i, done: false } : { value: 'inner done', done: true }; },
  };
  return { [Symbol.iterator]() { return it; } };
}
function* outer() {
  try {
    var r = yield* bare();
    log.push('result ' + r);
    yield 'after';
  } finally {
    log.push('outer finally');
  }
}
var out = [];
var g1 = outer();
out.push(g1.next('ignored'), g1.next('a'), g1.return('stop'), g1.next());
var g2 = outer();
g2.next();
try { g2.throw(new RangeError('into bare')); } catch (e) { out.push('throw: ' + e.name); }
out.push(g2.next());
out.push([...outer()]);
globalThis.__result = JSON.stringify([out, log]);
