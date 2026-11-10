class Base {
  #secret = 1;
  static created = 0;
  constructor(value) { this.value = value; Base.created++; }
  get secret() { return this.#secret; }
  describe() { return 'base:' + this.value; }
}
class Derived extends Base {
  constructor() { super(2); }
  describe() { return 'derived/' + super.describe(); }
}
new Derived().describe();
