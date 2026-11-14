// Two closures over the same binding see each other's writes.
function make() {
  let v = 0;
  return [function () { v += 1; return v; }, function () { v *= 10; return v; }];
}
var pair = make();
var out = [pair[0](), pair[1](), pair[0](), pair[1]()];
globalThis.__result = JSON.stringify(out);
