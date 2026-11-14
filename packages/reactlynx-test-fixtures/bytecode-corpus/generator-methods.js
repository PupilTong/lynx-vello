// Generator methods in object literals and classes, including computed names.
var o = { *items() { yield* [1, 2]; }, *['gen' + 'Name']() { yield this.v; }, v: 'v' };
class C { constructor() { this.data = [3, 4]; } *[Symbol.iterator]() { yield* this.data; } static *s() { yield 's'; } }
globalThis.__result = JSON.stringify([[...o.items()], [...o.genName()], [...new C()], [...C.s()]]);
