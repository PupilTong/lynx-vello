// Computed method, accessor and static names, evaluated once in order.
var log = [];
function name(n) { log.push(n); return n; }
class C {
  [name('m1')]() { return 1; }
  static [name('s1')]() { return 2; }
  get [name('g1')]() { return 3; }
  [Symbol.toPrimitive]() { return 42; }
}
var c = new C();
globalThis.__result = JSON.stringify([c.m1(), C.s1(), c.g1, +c, log]);
