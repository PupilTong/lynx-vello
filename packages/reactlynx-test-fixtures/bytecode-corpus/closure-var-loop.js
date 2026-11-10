// var in a for loop is one binding shared by every closure.
var fns = [];
for (var i = 0; i < 3; i++) fns.push(function () { return i; });
var fixed = [];
for (var j = 0; j < 3; j++) fixed.push((function (k) { return function () { return k; }; })(j));
globalThis.__result = JSON.stringify([fns.map(function (f) { return f(); }), fixed.map(function (f) { return f(); })]);
