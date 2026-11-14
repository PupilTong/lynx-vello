// Destructuring assignment and calls inside with: each target name and callee is
// looked up on the with object first.
var o = { a: 'obj a', describe() { return 'method of o, this is o: ' + (this === o); } };
var b;
function describe() { return 'global describe'; }
function f(x, y) {
  var z;
  var calls = [];
  with (o) {
    [a, x, z] = ['A', 'X', 'Z'];
    ({ p: b, q: y } = { p: 'B', q: 'Y' });
    calls.push(describe());
  }
  var read = function () {
    with (o) { [z, y] = [z + '!', y + '!']; return [x, y, z]; }
  };
  return [x, y, z, calls, read()];
}
var r = f();
o.x = 'obj x';
delete o.describe;
var r2 = f();
globalThis.__result = JSON.stringify([r, r2, o, b]);
