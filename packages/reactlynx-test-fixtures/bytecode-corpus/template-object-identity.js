// The strings array of a tagged template site is the same object on every evaluation.
function tag(strings) { return strings; }
function site() { return tag`same ${1} site`; }
var first = site(), second = site();
var other = tag`same ${1} site`;
var loop = [];
for (var i = 0; i < 2; i++) loop.push(tag`loop`);
globalThis.__result = JSON.stringify([first === second, first === other, loop[0] === loop[1]]);
