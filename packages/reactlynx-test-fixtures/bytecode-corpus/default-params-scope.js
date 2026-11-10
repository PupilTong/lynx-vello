// Default parameter expressions see earlier parameters but not the body's vars.
var x = 'outer';
function f(a = x, b = () => a) {
  var x = 'body';
  a = 'changed';
  return [a, b(), x];
}
function g(a = b, b) { return a; }
var err;
try { g(undefined, 1); } catch (e) { err = e.name; }
globalThis.__result = JSON.stringify([f(), f('given'), err]);
