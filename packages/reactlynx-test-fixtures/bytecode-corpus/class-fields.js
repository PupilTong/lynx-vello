// Public instance fields: initialisation order, this, computed names, arrow fields.
// The initialisers make no calls: tasm 0.0.53 crashes compiling a call in a field initialiser.
var log = [];
var k = 'computed';
class F {
  a = (log[log.length] = 'a', 1);
  b = this.a + 1;
  [k] = 'c';
  arrow = () => this.b;
  noInit;
  constructor() { log[log.length] = 'ctor ' + this.b; }
}
var f = new F();
var detached = f.arrow;
globalThis.__result = JSON.stringify([f.a, f.b, f.computed, detached(), 'noInit' in f, Object.keys(f), log]);
