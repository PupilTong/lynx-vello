// for-in over an array gives index strings and extra properties.
var arr = ['x', , 'z'];
arr.extra = true;
var out = [];
for (var i in arr) out.push(typeof i + ':' + i);
var sum = 0;
for (let i in [5, 6]) sum += i;
globalThis.__result = JSON.stringify([out, sum]);
