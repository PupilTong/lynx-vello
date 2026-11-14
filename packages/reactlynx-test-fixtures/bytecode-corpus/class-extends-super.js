// extends with super() in the constructor and super.method() in methods.
class Animal {
  constructor(name) { this.name = name; }
  speak() { return this.name + ' makes a sound'; }
  static kind() { return 'animal'; }
}
class Dog extends Animal {
  constructor(name) { super(name + '!'); this.tricks = []; }
  speak() { return super.speak() + ' (woof)'; }
  static kind() { return 'dog < ' + super.kind(); }
}
var d = new Dog('rex');
globalThis.__result = JSON.stringify([d.speak(), Dog.kind(), d instanceof Animal, d.name, Object.getPrototypeOf(Dog.prototype) === Animal.prototype]);
