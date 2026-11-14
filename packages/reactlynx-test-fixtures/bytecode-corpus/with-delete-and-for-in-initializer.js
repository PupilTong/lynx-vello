// delete of a bare name inside with removes the with object's property first;
// a for-in var initializer (sloppy mode) inside with assigns through the with object.
var target = { a: 1, b: 2, k: 'k on target' };
var out = [];
created = 'global';
var declared = 'var';
with (target) {
  out.push(delete a, 'a' in target);
  out.push(delete created, typeof created);
  out.push(delete declared, typeof declared);
  out.push(delete notAnywhere);
  for (var k = 'initial' in {}) ;
  out.push(k, target.k);
  for (var m = 'm init' in { p: 1 }) out.push(m);
}
out.push(m);
function inFunction(obj) {
  var local = 'local';
  var r;
  with (obj) {
    r = [delete local, delete hidden, typeof hidden];
    for (var j = 'j init' in {}) ;
  }
  return [r, j, local, obj.j];
}
out.push(inFunction({ hidden: 1 }), inFunction({ hidden: 2, j: 'obj j' }));
globalThis.__result = JSON.stringify(out);
