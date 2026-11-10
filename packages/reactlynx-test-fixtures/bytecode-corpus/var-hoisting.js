// var declarations are hoisted to the function or script with value undefined.
var before = typeof hoisted;
var hoisted = 1;
function f() {
  var r = [typeof local];
  if (false) { var local = 1; }
  r.push(typeof local);
  for (var i = 0; i < 2; i++) { var inLoop = i; }
  r.push(i, inLoop);
  return r;
}
globalThis.__result = JSON.stringify([before, hoisted, f()]);
