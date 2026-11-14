// super property reads, writes and calls with arrow functions and computed keys.
class A { get v() { return 'A.v'; } m() { return 'A.m ' + this.tag; } }
class B extends A {
  constructor() { super(); this.tag = 'b'; }
  get v() { return 'B>' + super.v; }
  m() { var key = 'm'; var arrow = () => super[key](); return 'B.m ' + arrow(); }
  set() { super.written = 1; return this.hasOwnProperty('written'); }
}
var b = new B();
globalThis.__result = JSON.stringify([b.v, b.m(), b.set()]);
