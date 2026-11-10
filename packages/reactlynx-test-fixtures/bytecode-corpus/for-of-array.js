// for-of over arrays, typed arrays and array iterators.
var out = [];
for (var v of [1, 2, 3]) out.push(v);
for (const v of [, 'hole']) out.push(String(v));
for (let [i, v] of ['a', 'b'].entries()) out.push(i + v);
for (const k of ['p', 'q'].keys()) out.push(k);
for (const n of new Uint8Array([7, 8])) out.push(n);
var arr = [1];
for (const v of arr) { if (arr.length < 4) arr.push(v + 1); out.push('grow' + v); }
globalThis.__result = JSON.stringify(out);
