// A closure called before the captured let is initialised throws; after, it reads the value.
var out = [];
function read() { return captured; }
try { out.push(read()); } catch (e) { out.push(e.name); }
let captured = 'ready';
out.push(read());
function outer() {
  var g = function () { return v; };
  var r;
  try { r = g(); } catch (e) { r = e.name; }
  let v = 5;
  return [r, g()];
}
out.push(outer());
globalThis.__result = JSON.stringify(out);
