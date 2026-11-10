// new.target in functions and class constructors.
function F() { return new.target === undefined ? 'call' : new.target === F ? 'new F' : 'other'; }
class A { constructor() { this.target = new.target.name; } }
class B extends A {}
function G() { var arrow = () => new.target; return arrow(); }
globalThis.__result = JSON.stringify([F(), new F() instanceof F, Reflect.construct(F, []) instanceof F, new A().target, new B().target, G() === undefined, new G() instanceof G]);
