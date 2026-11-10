// Generator objects: next results, return value and completion.
function* g() { var x = yield 1; yield x * 2; return 'end'; }
var it = g();
var out = [it.next('ignored'), it.next(5), it.next(), it.next()];
function* empty() {}
out.push(empty().next());
out.push(Object.prototype.toString.call(it), typeof it[Symbol.iterator]);
globalThis.__result = JSON.stringify(out);
