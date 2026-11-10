// Array length grows with index writes and truncates when assigned.
var a = [1, 2, 3];
a[10] = 'x';
var len1 = a.length;
a.length = 2;
var arr2 = [];
arr2.length = 3;
var err;
try { a.length = -1; } catch (e) { err = e.name; }
var b = [1, 2, 3];
b.length = 0;
globalThis.__result = JSON.stringify([len1, a, arr2.length, 0 in arr2, err, b, Array(3).fill(0)]);
