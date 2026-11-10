// Class getters and setters, instance and static.
class Temp {
  constructor() { this._c = 0; }
  get f() { return this._c * 9 / 5 + 32; }
  set f(v) { this._c = (v - 32) * 5 / 9; }
  static get unit() { return 'C'; }
}
var t = new Temp();
t.f = 212;
var d = Object.getOwnPropertyDescriptor(Temp.prototype, 'f');
globalThis.__result = JSON.stringify([t._c, t.f, Temp.unit, typeof d.get, typeof d.set, d.enumerable]);
