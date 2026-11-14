// typeof compared with string literals, with ===, !==, == and !=, on locals,
// arguments, captured variables, globals and undeclared names.
var values = [function () {}, class {}, () => 1, async function () {}, {}, null, undefined, 1, 'f'];
function classify(v) {
  var out = [];
  if (typeof v === 'function') out.push('fn');
  if (typeof v !== 'function') out.push('not fn');
  if (typeof v == 'undefined') out.push('undef');
  if (typeof v != 'object') out.push('not object');
  return out.join(',');
}
function captured(v) { return function () { return typeof v === 'function' ? 'cfn' : typeof v; }; }
var g = function () {};
var r = [
  values.map(classify),
  values.map(function (v) { return captured(v)(); }),
  typeof g === 'function',
  typeof undeclaredName === 'function',
  typeof undeclaredName === 'undefined',
  typeof Symbol() === 'symbol',
];
globalThis.__result = JSON.stringify(r);
