// Arrow functions have no own arguments; they see the enclosing function's.
function outer() {
  var inner = () => arguments[0] + arguments.length;
  return inner('ignored', 'also');
}
var arrowWithRest = (...a) => a.length;
globalThis.__result = JSON.stringify([outer(10, 20, 30), arrowWithRest(1, 2)]);
