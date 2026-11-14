// Getters and setters in object literals that use super, with the receiver kept as
// this, compound assignment through accessors and computed accessor names.
var log = [];
var base = {
  _store: 'base store',
  get value() { log.push('base get'); return this._store; },
  set value(v) { log.push('base set ' + v); this._store = v; },
  greet() { return 'base greet ' + this.name; },
};
var key = 'dyn';
var derived = {
  __proto__: base,
  name: 'derived',
  get value() { return 'derived(' + super.value + ')'; },
  set value(v) { super.value = v + '!'; },
  get [key + 'Getter']() { return super.greet(); },
  set [key + 'Setter'](v) { super.value = 'via computed ' + v; },
  get arrow() { var f = () => super.greet(); return f(); },
};
var out = [derived.value];
derived.value = 'x';
out.push(derived.value, base._store, derived._store, Object.prototype.hasOwnProperty.call(derived, '_store'));
derived.value += 'y';
out.push(derived._store, derived.dynGetter, derived.arrow);
derived.dynSetter = 'z';
out.push(derived._store);
var other = { name: 'other' };
Object.setPrototypeOf(other, derived);
out.push(other.value, other.dynGetter);
globalThis.__result = JSON.stringify([out, log]);
