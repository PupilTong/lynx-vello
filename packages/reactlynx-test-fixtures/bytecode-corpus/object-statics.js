// Object static methods: keys, values, entries, assign, freeze, seal, create.
var o = { a: 1, b: 2 };
var target = Object.assign({}, o, { c: 3 }, null);
var frozen = Object.freeze({ x: 1 });
frozen.x = 2;
var sealed = Object.seal({ y: 1 });
sealed.y = 2; sealed.z = 3; delete sealed.y;
var created = Object.create({ p: 1 }, { own: { value: 2, enumerable: true } });
globalThis.__result = JSON.stringify([Object.keys(o), Object.values(o), Object.entries(o), target, frozen.x, Object.isFrozen(frozen), sealed, Object.isSealed(sealed), created.p, Object.keys(created), Object.getOwnPropertyNames('ab')]);
