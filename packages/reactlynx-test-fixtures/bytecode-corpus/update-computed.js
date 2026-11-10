// ++/-- on computed members evaluate object and key once; the log records each key conversion.
var log = [];
var arr = [10, 20, 30];
var o = { a: 1 };
function k() { log.push('k'); return 'a'; }
var i = 0;
var r1 = arr[i++]++;
var r2 = ++arr[i];
var r3 = o[k()]--;
var r4 = --o[k()];
var keyObj = { toString() { log.push('toString'); return 'a'; } };
var r5 = o[keyObj]++;
globalThis.__result = JSON.stringify([r1, r2, r3, r4, r5, arr, o, i, log]);
