// Inner declarations shadow outer ones, including parameters and catch bindings.
var v = 'global';
function f(v) {
  var out = [v];
  { let v = 'block'; out.push(v); }
  try { throw 'caught'; } catch (v) { out.push(v); }
  out.push(v);
  return out;
}
function g() { var v = 'local'; return [v, globalThis.v]; }
globalThis.__result = JSON.stringify([f('param'), g(), v]);
