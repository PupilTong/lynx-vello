// A closure keeps its captured local alive between calls.
function counter(start) {
  var n = start;
  return { inc: function () { return ++n; }, get: function () { return n; } };
}
var a = counter(0), b = counter(10);
a.inc(); a.inc(); b.inc();
globalThis.__result = JSON.stringify([a.get(), b.get(), a.inc()]);
