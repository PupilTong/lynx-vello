// Private methods and accessors.
class P {
  #v = 1;
  #double() { return this.#v * 2; }
  get #tripled() { return this.#v * 3; }
  set #value(x) { this.#v = x; }
  run(x) { this.#value = x; return [this.#double(), this.#tripled]; }
  static callOn(o) { try { return o.#double(); } catch (e) { return e.name; } }
}
var p = new P();
globalThis.__result = JSON.stringify([p.run(5), P.callOn(p), P.callOn({})]);
