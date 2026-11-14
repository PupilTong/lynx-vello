// The arguments object: length, indexing, iteration, toString tag.
function f() {
  var out = [arguments.length, arguments[0], arguments[5] === undefined, Object.prototype.toString.call(arguments)];
  var sum = 0;
  for (var v of arguments) sum += v;
  out.push(sum, Array.isArray(arguments), Array.from(arguments).length);
  return out;
}
globalThis.__result = JSON.stringify([f(1, 2, 3), f()]);
