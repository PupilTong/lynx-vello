// Async methods in classes and object literals, static and with super.
class Base { async value() { return 'base'; } }
class Derived extends Base {
  async value() { return (await super.value()) + '+derived'; }
  static async make() { return new Derived(); }
  async *items() { yield 1; yield 2; }
}
var o = { async m() { return this === o; } };
async function main() {
  var d = await Derived.make();
  var items = [];
  for await (const v of d.items()) items.push(v);
  return [await d.value(), await o.m(), items];
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
