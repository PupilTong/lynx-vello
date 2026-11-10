// yield* over a hand-written iterator forwards next arguments.
var log = [];
var iterable = {
  [Symbol.iterator]() {
    var i = 0;
    return { next(v) { log.push('next ' + v); i++; return i <= 2 ? { value: i, done: false } : { value: 'ret', done: true }; } };
  },
};
function* g() { var r = yield* iterable; yield r; }
var it = g();
var out = [it.next('a').value, it.next('b').value, it.next('c').value, it.next('d').done];
globalThis.__result = JSON.stringify([out, log]);
