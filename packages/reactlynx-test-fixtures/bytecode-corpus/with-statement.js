// with statements in sloppy code resolve names through the object first.
var o = { a: 1, b: 2 };
var a = 'global a', c = 'global c';
var out = [];
with (o) {
  out.push(a, b, c);
  a = 10;
  c = 'set c';
  var declaredInWith = 'v';
}
function f(obj) { with (obj) { return function () { return x; }; } }
var reader = f({ x: 'captured x' });
with ({ fn() { return this === undefined ? 'undefined' : typeof this; } }) out.push(fn());
globalThis.__result = JSON.stringify([out, o.a, a, c, declaredInWith, reader()]);
