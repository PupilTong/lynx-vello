// Labeled continue to an outer loop.
var out = [];
rows: for (var i = 0; i < 4; i++) {
  for (var j = 0; j < 4; j++) {
    if (j > i) continue rows;
    if ((i + j) % 2) continue;
    out.push(i + '' + j);
  }
}
var n = 0;
outerWhile: while (n < 3) {
  n++;
  for (const x of [1, 2, 3]) { if (x === 2) continue outerWhile; out.push('n' + n + x); }
}
globalThis.__result = JSON.stringify(out);
