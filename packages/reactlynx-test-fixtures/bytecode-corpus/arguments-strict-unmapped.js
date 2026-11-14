// In strict code arguments is not mapped and callee throws.
function f(a) {
  'use strict';
  arguments[0] = 'changed';
  a = 'param';
  var callee;
  try { callee = arguments.callee; } catch (e) { callee = e.name; }
  return [a, arguments[0], callee];
}
globalThis.__result = JSON.stringify(f(1));
