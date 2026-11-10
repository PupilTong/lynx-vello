// super() called from arrow functions inside a derived constructor initialises the
// constructor's this; a second call throws, and reading this before super() throws.
class Base { constructor(v) { this.v = v; } }
class Direct extends Base {
  constructor() { const init = () => super('arrow'); init(); this.after = this.v; }
}
class Nested extends Base {
  constructor() {
    const outer = () => { const inner = () => super('nested'); return inner(); };
    const r = outer();
    this.same = r === this;
  }
}
class Twice extends Base {
  constructor() {
    const init = () => super('once');
    init();
    try { init(); } catch (e) { this.second = e.name; }
  }
}
class Early extends Base {
  constructor() {
    const read = () => this;
    let r;
    try { read(); } catch (e) { r = e.name; }
    super('late');
    this.early = r;
    this.readAfter = read() === this;
  }
}
class Conditional extends Base {
  constructor(flag) {
    const a = () => super('a'), b = () => super('b');
    if (flag) a(); else b();
  }
}
class Never extends Base { constructor() { const init = () => super('never'); } }
var out = [new Direct(), new Nested(), new Twice(), new Early(), new Conditional(true), new Conditional(false)];
try { new Never(); } catch (e) { out.push(e.name); }
globalThis.__result = JSON.stringify(out);
