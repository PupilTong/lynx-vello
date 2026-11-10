// Private instance fields: read, write, per-instance and brand checks.
class Counter {
  #count = 0;
  #step;
  constructor(step) { this.#step = step; }
  inc() { this.#count += this.#step; return this.#count; }
  static read(o) { try { return o.#count; } catch (e) { return e.name; } }
  equals(other) { return this.#count === other.#count; }
}
var a = new Counter(2), b = new Counter(5);
a.inc(); a.inc(); b.inc();
globalThis.__result = JSON.stringify([a.inc(), b.inc(), Counter.read(a), Counter.read({}), a.equals(b), Object.keys(a)]);
