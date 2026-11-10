// Object literal shorthand properties and methods.
var x = 1, y = 'two';
var o = { x, y, z() { return this.x + 1; }, 'quoted key': 3, 42: 'num', get g() { return 'got'; } };
globalThis.__result = JSON.stringify([o.x, o.y, o.z(), o['quoted key'], o[42], o.g, Object.keys(o)]);
