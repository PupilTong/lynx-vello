// Closures passed as callbacks capture loop and outer state.
function makeAdders(list) { return list.map(function (n) { return function (x) { return x + n; }; }); }
var adders = makeAdders([1, 10, 100]);
var total = 0;
[1, 2, 3].forEach(function (v) { total += v; });
var memo = (function () { var cache = {}; return function (n) { if (n in cache) return 'hit ' + cache[n]; cache[n] = n * n; return 'miss ' + cache[n]; }; })();
globalThis.__result = JSON.stringify([adders.map(function (f) { return f(1); }), total, memo(3), memo(3), memo(4)]);
