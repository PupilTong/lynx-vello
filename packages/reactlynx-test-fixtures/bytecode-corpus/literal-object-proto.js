// __proto__ in an object literal sets the prototype; a computed one does not.
var base = { inherited: 'yes' };
var o = { __proto__: base, own: 1 };
var p = { ['__proto__']: 5 };
var __proto__ = 7;
var q = { __proto__ };
globalThis.__result = JSON.stringify([o.inherited, Object.getPrototypeOf(o) === base, Object.keys(o), p.__proto__ === 5, Object.getPrototypeOf(p) === Object.prototype, q.__proto__]);
