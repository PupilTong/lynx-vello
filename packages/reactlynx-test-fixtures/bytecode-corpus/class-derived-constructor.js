// Derived constructors: this before super throws, returning an object replaces this, returning a primitive throws.
class Base {}
class EarlyThis extends Base { constructor() { var t; try { t = this; } catch (e) { t = e.name; } super(); this.early = t; } }
class ReturnsObject extends Base { constructor() { super(); return { replaced: true }; } }
class ReturnsObjectNoSuper extends Base { constructor() { return { noSuper: true }; } }
class ReturnsPrimitive extends Base { constructor() { super(); return 1; } }
class NoSuper extends Base { constructor() {} }
var out = [new EarlyThis().early, new ReturnsObject(), new ReturnsObjectNoSuper()];
try { new ReturnsPrimitive(); } catch (e) { out.push(e.name); }
try { new NoSuper(); } catch (e) { out.push(e.name); }
class DoubleSuper extends Base { constructor() { super(); try { super(); } catch (e) { this.second = e.name; } } }
out.push(new DoubleSuper().second);
globalThis.__result = JSON.stringify(out);
