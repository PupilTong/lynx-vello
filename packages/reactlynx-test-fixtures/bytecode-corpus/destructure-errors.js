// Destructuring null, undefined or a non-iterable throws a TypeError.
var out = [];
try { var { a } = null; } catch (e) { out.push(e.name); }
try { var [b] = undefined; } catch (e) { out.push(e.name); }
try { var [c] = {}; } catch (e) { out.push(e.name); }
try { var [d] = 5; } catch (e) { out.push(e.name); }
try { (function ({ p }) {})(); } catch (e) { out.push(e.name); }
var { length } = 'ok';
out.push(length);
globalThis.__result = JSON.stringify(out);
