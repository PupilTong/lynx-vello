// while and do-while loops, including a do-while that runs once.
var i = 0, s = 0;
while (i < 10) { s += i; i++; }
var j = 100, runs = 0;
do { runs++; } while (j < 10);
var k = 0;
do k++; while (k < 5);
var n = 27, steps = 0;
while (n !== 1) { n = n % 2 ? 3 * n + 1 : n / 2; steps++; }
globalThis.__result = JSON.stringify([s, runs, k, steps]);
