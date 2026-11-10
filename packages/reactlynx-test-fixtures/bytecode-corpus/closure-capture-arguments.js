// Closures capturing parameters and the arguments object.
function f(x, y) {
  var getX = function () { return x; };
  var getArgs = function () { return Array.prototype.slice.call(arguments).concat([arguments.length]); };
  var outerArgs = arguments;
  var readOuter = function () { return outerArgs[1]; };
  x = x + 100;
  return [getX(), getArgs(7, 8), readOuter()];
}
globalThis.__result = JSON.stringify(f(1, 2, 3));
