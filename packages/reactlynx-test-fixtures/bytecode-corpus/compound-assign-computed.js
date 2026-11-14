// a[b] op= c evaluates a and b once; the log records each key conversion.
var log = [];
var o = { k1: 1, k2: 10 };
var keyObj = { toString: function () { log.push('toString'); return 'k1'; } };
function getObj() { log.push('obj'); return o; }
function getKey() { log.push('key'); return 'k2'; }
o[keyObj] += 5;
getObj()[getKey()] -= 3;
var arr = [1, 2, 3];
var i = 0;
arr[i++] += 10;
arr[i++] *= 10;
globalThis.__result = JSON.stringify([o, arr, i, log]);
