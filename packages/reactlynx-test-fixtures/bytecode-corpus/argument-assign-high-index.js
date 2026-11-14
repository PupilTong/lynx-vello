// Assignments to the fourth and later parameters, in sloppy functions with mapped
// arguments, in strict functions and in functions with default parameters.
function sloppy(a, b, c, d, e, f) {
  d = 'D';
  e = 'E';
  var g = (f = 'F');
  f += '!';
  return [a, b, c, d, e, f, g, arguments[3], arguments[4], arguments[5], arguments.length];
}
function strict(a, b, c, d, e, f) {
  'use strict';
  d = 'D';
  e = 'E';
  var g = (f = 'F');
  return [d, e, f, g, arguments[3], arguments[4], arguments.length];
}
function defaults(a, b, c, d, e = 'e default', f) {
  e += '+';
  f = e;
  d = f + d;
  return [d, e, f, arguments.length];
}
function counter(a, b, c, d, e) {
  for (e = 0; e < 3; e++) d = (d || 0) + e;
  return [d, e];
}
globalThis.__result = JSON.stringify([
  sloppy(1, 2, 3, 4, 5, 6),
  sloppy(1, 2, 3, 4),
  strict(1, 2, 3, 4, 5, 6),
  defaults(1, 2, 3, 'd'),
  counter(),
]);
