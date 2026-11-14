// extends a non-constructor or a bad prototype throws TypeError; extends null works.
var out = [];
try { class A extends 5 {} } catch (e) { out.push(e.name); }
try { var arrow = () => {}; class B extends arrow {} } catch (e) { out.push(e.name); }
try { function F() {} F.prototype = 3; class C extends F {} } catch (e) { out.push(e.name); }
class N extends null {}
out.push(Object.getPrototypeOf(N.prototype) === null);
try { new N(); } catch (e) { out.push(e.name); }
globalThis.__result = JSON.stringify(out);
