// Default parameter values, applied for undefined only and evaluated per call.
var calls = 0;
function next() { return ++calls; }
function f(a, b = a * 2, c = next(), d = [a, b]) { return [a, b, c, d]; }
globalThis.__result = JSON.stringify([f(1), f(1, undefined), f(1, null), f(1, 5, 9), calls, f.length]);
