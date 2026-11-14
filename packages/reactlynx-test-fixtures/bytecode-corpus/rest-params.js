// Rest parameters collect the remaining arguments into an array.
function f(a, ...rest) { return [a, rest, Array.isArray(rest), rest.length]; }
var g = (...all) => all.reverse();
function h(a, b, ...c) { return arguments.length + ':' + c.length; }
globalThis.__result = JSON.stringify([f(1, 2, 3), f(), g(1, 2, 3), h(1), h(1, 2, 3, 4), f.length, h.length]);
