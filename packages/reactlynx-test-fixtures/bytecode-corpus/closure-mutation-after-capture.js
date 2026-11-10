// A closure reads the current value of a binding, not the value at capture.
function f() {
  var x = 1;
  var read = function () { return x; };
  x = 2;
  var r1 = read();
  x = 3;
  return [r1, read()];
}
var gx = 'before';
var readG = function () { return gx; };
gx = 'after';
globalThis.__result = JSON.stringify([f(), readG()]);
