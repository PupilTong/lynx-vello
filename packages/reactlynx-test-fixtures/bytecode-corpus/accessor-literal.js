// Getters and setters declared in object literals.
var log = [];
var o = {
  _v: 1,
  get v() { log.push('get'); return this._v; },
  set v(x) { log.push('set ' + x); this._v = x * 2; },
  get ['comp' + 'uted']() { return 'c'; },
};
o.v = 5;
var r = o.v;
o.v += 1;
globalThis.__result = JSON.stringify([r, o._v, o.computed, log]);
