// Object spread copies own enumerable properties; later keys win.
var base = { a: 1, b: 2 };
var proto = Object.create({ inherited: 1 });
proto.own = 2;
var hidden = Object.defineProperty({}, 'h', { value: 1, enumerable: false });
var log = [];
var withGetter = { get g() { log.push('get'); return 'G'; } };
var o = { ...base, b: 3, ...proto, ...hidden, ...null, ...undefined, ...'xy', ...withGetter };
globalThis.__result = JSON.stringify([o, log, Object.getOwnPropertyDescriptor(o, 'g').value]);
