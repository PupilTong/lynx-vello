// Closures created in loops over let, const and var bindings, with continue and
// break (labeled and unlabeled) leaving iterations before and after the capture.
var fns = [];
for (let i = 0; i < 6; i++) {
  if (i === 1) continue;
  let doubled = i * 2;
  fns.push(() => i + ':' + doubled);
  if (i === 4) break;
  i += 0;
}
var fromWhile = [];
let w = 0;
while (w < 5) {
  const snapshot = w++;
  if (snapshot % 2) { fromWhile.push(() => 'odd ' + snapshot); continue; }
  fromWhile.push(() => 'even ' + snapshot + ' now ' + w);
}
var labeled = [];
outer: for (let x = 0; x < 3; x++) {
  for (const y of ['a', 'b', 'c']) {
    if (y === 'b') continue outer;
    labeled.push(() => x + y);
    if (x === 2) break outer;
  }
}
var vars = [];
for (var v = 0; v < 3; v++) {
  if (v === 1) continue;
  vars.push(function () { return v; });
}
var forIn = [];
for (let k in { p: 1, q: 2, r: 3 }) {
  if (k === 'q') continue;
  forIn.push(() => k);
}
var call = function (f) { return f(); };
globalThis.__result = JSON.stringify([fns.map(call), fromWhile.map(call), labeled.map(call), vars.map(call), forIn.map(call)]);
