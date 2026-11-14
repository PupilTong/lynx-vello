// let in a for loop gives each iteration its own binding.
var fns = [];
for (let i = 0; i < 3; i++) { fns.push(function () { return i; }); }
var fns2 = [];
for (let i = 0; i < 3; i++) { fns2.push(() => i); i++; }
var fns3 = [];
for (const k of ['a', 'b']) fns3.push(() => k);
var fns4 = [];
for (const k in { x: 1, y: 2 }) fns4.push(() => k);
globalThis.__result = JSON.stringify([fns.map(f => f()), fns2.map(f => f()), fns3.map(f => f()), fns4.map(f => f())]);
