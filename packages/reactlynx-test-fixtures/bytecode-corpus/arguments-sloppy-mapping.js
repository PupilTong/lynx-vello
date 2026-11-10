// In sloppy code with simple parameters, arguments is mapped to the parameters.
function f(a, b) {
  arguments[0] = 'changed';
  b = 'param';
  return [a, arguments[1], arguments.length];
}
function g(a, b = 2) {
  arguments[0] = 'changed';
  return [a, arguments[0]];
}
globalThis.__result = JSON.stringify([f(1, 2), f(1), g(1)]);
