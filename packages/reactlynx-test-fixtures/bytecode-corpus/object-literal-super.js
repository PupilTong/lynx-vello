// super in object literal methods uses the home object's prototype.
var base = { hello() { return 'base hello ' + this.name; }, get v() { return 'base v'; } };
var obj = {
  __proto__: base,
  name: 'obj',
  hello() { return 'obj > ' + super.hello(); },
  get v() { return 'obj > ' + super.v; },
};
var other = { name: 'other', hello: obj.hello };
Object.setPrototypeOf(other, { hello() { return 'never'; } });
globalThis.__result = JSON.stringify([obj.hello(), obj.v, other.hello()]);
