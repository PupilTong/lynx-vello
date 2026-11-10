// Function.prototype.call, apply and bind set this and arguments.
function show(a, b) { return [this && this.tag, a, b]; }
var o = { tag: 'o' };
var bound = show.bind(o, 'first');
var rebound = bound.bind({ tag: 'ignored' }, 'second');
function Ctor(x) { this.x = x; }
var BoundCtor = Ctor.bind(null, 5);
var inst = new BoundCtor();
globalThis.__result = JSON.stringify([show.call(o, 1, 2), show.apply(o, [3, 4]), bound('x'), rebound(), inst.x, inst instanceof Ctor]);
