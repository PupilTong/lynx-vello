// instanceof walks the prototype chain and honours Symbol.hasInstance.
function A() {}
function B() {}
B.prototype = Object.create(A.prototype);
var b = new B();
var Even = { [Symbol.hasInstance]: function (n) { return n % 2 === 0; } };
var err;
try { b instanceof {}; } catch (e) { err = e.name; }
var bound = A.bind(null);
globalThis.__result = JSON.stringify([b instanceof B, b instanceof A, b instanceof Object, [] instanceof Array, 2 instanceof Even, 3 instanceof Even, err, new A() instanceof bound, Object.create(null) instanceof Object]);
