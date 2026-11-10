// Reads walk the prototype chain; writes create own properties unless a setter exists.
var log = [];
var proto = { a: 1, set s(v) { log.push('setter ' + v); } };
var o = Object.create(proto);
var before = o.a;
o.a = 2;
o.s = 3;
var ro = Object.create(Object.defineProperty({}, 'fixed', { value: 1, writable: false }));
ro.fixed = 2;
globalThis.__result = JSON.stringify([before, o.a, proto.a, o.hasOwnProperty('a'), o.hasOwnProperty('s'), ro.fixed, ro.hasOwnProperty('fixed'), log]);
