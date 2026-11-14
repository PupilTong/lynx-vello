// Static methods and this inside them, inherited by subclasses.
class A {
  static create() { return new this(); }
  static describe() { return 'static of ' + this.name; }
}
class B extends A {}
globalThis.__result = JSON.stringify([A.describe(), B.describe(), B.create() instanceof B, Object.getPrototypeOf(B) === A, typeof new A().describe]);
