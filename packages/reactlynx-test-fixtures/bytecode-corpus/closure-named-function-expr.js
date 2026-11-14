// A named function expression binds its own name, read-only, inside its body.
var f = function inner(n) {
  inner = null;
  return n > 0 ? inner.length + n : typeof inner;
};
var g = function self() { 'use strict'; try { self = 1; } catch (e) { return e.name; } };
globalThis.__result = JSON.stringify([f(0), f(2), typeof inner, g()]);
