// Compound assignment and update expressions on names inside with: each name is
// looked up on the with object first, then as an argument, a local, a captured
// variable or a global.
var scope = { onObj: 1 };
var g = 10;
function f(arg) {
  var local = 100;
  var log = [];
  with (scope) {
    onObj += 5;
    arg += 2;
    local *= 3;
    g -= 1;
    log.push(onObj++, ++arg, local--, g++);
  }
  var inner = function () {
    with (scope) {
      local += 7;
      onObj <<= 1;
      arg = arg * 10;
    }
    return local;
  };
  log.push(inner(), local, arg, scope.onObj, g);
  return log;
}
var first = f(1);
scope.arg = 50;
scope.local = 'on scope';
var second = f(1);
globalThis.__result = JSON.stringify([first, second, scope, g]);
