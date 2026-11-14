// Inherited getters receive the original receiver as this.
var proto = { get full() { return this.first + ' ' + this.last; }, set full(v) { var p = v.split(' '); this.first = p[0]; this.last = p[1]; } };
var person = Object.create(proto);
person.full = 'Ada Lovelace';
var other = Object.create(person);
other.first = 'Grace';
globalThis.__result = JSON.stringify([person.full, other.full, Object.keys(person), Object.keys(other)]);
