// Classic for loops with empty clauses, multiple variables and break.
var out = [];
for (var i = 0, j = 5; i < j; i++, j--) out.push(i * j);
var k = 0;
for (;;) { if (++k > 3) break; }
out.push(k);
for (let a = 3; a; a--) out.push('a' + a);
var count = 0;
for (var x = 0; x < 100; x += 7) count++;
out.push(count);
globalThis.__result = JSON.stringify(out);
