// Object destructuring with computed keys into member targets, with the order in
// which keys, target objects and target keys are evaluated.
var src = { k1: 'one', k2: 'two', k3: 'three' };
var target = {}, arr = [];
var keys = ['k1', 'k2', 'k3'];
var i = 0;
({ [keys[i++]]: target.a, [keys[i++]]: target['b' + i], [keys[i++]]: arr[i] } = src);
var order = [];
function key(name) { order.push('key ' + name); return name; }
function obj(name) { order.push('obj ' + name); return target; }
({ [key('k1')]: obj('x')[key('x')], [key('missing')]: obj('y')[key('y')] = 'dflt' } = src);
function inFunction(source, k) {
  var local = {};
  ({ [k]: local[k + '!'], [k + 'x']: local.missing } = source);
  return local;
}
globalThis.__result = JSON.stringify([target, arr, i, order, inFunction({ q: 'Q' }, 'q')]);
