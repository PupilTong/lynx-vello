// Immediately invoked function expressions, named and arrow.
var a = (function (x) { return x * 2; })(21);
var b = (function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); })(5);
var c = ((x, y) => x + y)(1, 2);
var d = !function () { return 0; }();
var e = (function () { return typeof fact; })();
globalThis.__result = JSON.stringify([a, b, c, d, e]);
