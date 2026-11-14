// The comma operator evaluates left to right and yields the last value.
var log = [];
var x = (log.push('a'), log.push('b'), 42);
var i, j;
for (i = 0, j = 10; i < j; i += 3, j -= 3) log.push(i + ':' + j);
var y = (1, 2, 3);
globalThis.__result = JSON.stringify([x, y, log]);
