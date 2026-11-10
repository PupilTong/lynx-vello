// Destructuring in for-of and for-in heads.
var out = [];
for (var [k, v] of new Map([['a', 1], ['b', 2]])) out.push(k + v);
for (const { id, tags: [t] = ['none'] } of [{ id: 1, tags: ['x'] }, { id: 2 }]) out.push(id + t);
for (let [i, ch] of Array.from('xy').entries()) out.push(i + ch);
for (var [first] in { key1: 1, key2: 2 }) out.push(first);
globalThis.__result = JSON.stringify(out);
