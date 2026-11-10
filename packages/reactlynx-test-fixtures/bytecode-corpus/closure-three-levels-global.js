// Closures nested three levels deep read locals of every level and globals.
var g = 'G';
let gl = 'L';
function one(a) {
  var x = 'x';
  return function two(b) {
    var y = 'y';
    return function three(c) {
      return [g, gl, a, b, c, x, y, typeof globalThis.missingGlobal].join('');
    };
  };
}
var f = one('1')('2');
g = 'g';
globalThis.__result = JSON.stringify([f('3'), one('4')('5')('6')]);
