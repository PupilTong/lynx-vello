// Private static fields and methods.
class Registry {
  static #items = [];
  static #check(x) { return typeof x === 'string'; }
  static add(x) { if (Registry.#check(x)) Registry.#items.push(x); return Registry.#items.length; }
  static count() { return this.#items.length; }
}
class Sub extends Registry {}
var r = [Registry.add('a'), Registry.add(1), Registry.add('b'), Registry.count()];
try { Sub.count(); } catch (e) { r.push(e.name); }
globalThis.__result = JSON.stringify(r);
