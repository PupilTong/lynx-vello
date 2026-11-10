// super() inside try/catch/finally in derived constructors, and returns that cross
// a finally block.
var log = [];
class Base {
  constructor(fail) {
    if (fail) throw new TypeError('base');
    this.base = true;
  }
}
class InTry extends Base {
  constructor(fail) {
    try { super(fail); log.push('after super'); }
    catch (e) { log.push('caught ' + e.name); super(false); }
    finally { log.push('finally'); }
    this.done = true;
  }
}
class InFinally extends Base {
  constructor() {
    try { log.push('try'); } finally { super(false); }
    this.fin = true;
  }
}
class ReturnInFinally extends Base {
  constructor() {
    try { super(false); return; } finally { log.push('return crosses finally'); }
  }
}
class ReturnObject extends Base {
  constructor() { try { return { replaced: true }; } finally { log.push('object return'); } }
}
class NoSuperReturnUndefined extends Base {
  constructor() { try { return; } finally { log.push('no super'); } }
}
var out = [new InTry(false), new InTry(true), new InFinally(), new ReturnInFinally(), new ReturnObject()];
try { new NoSuperReturnUndefined(); } catch (e) { out.push(e.name); }
globalThis.__result = JSON.stringify([out, log]);
