// Accessors and methods are looked up along the class chain; fields shadow them.
class A { get x() { return 'A.x'; } m() { return 'A.m'; } }
class B extends A { m() { return 'B.m'; } }
class C extends B { x = 'field'; }
var b = new B(), c = new C();
globalThis.__result = JSON.stringify([b.x, b.m(), c.x, c.m(), Object.keys(c)]);
