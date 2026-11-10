'use strict';
// A script with a 'use strict' directive: undeclared assignment, this in functions, read-only writes.
var out = [];
try { notDeclaredHere = 1; } catch (e) { out.push(e.name); }
function thisValue() { return this; }
out.push(typeof thisValue());
try { Object.freeze([1]).push(2); } catch (e) { out.push(e.name); }
try { undefined = 1; } catch (e) { out.push(e.name); }
try { (function () { arguments.callee; })(); } catch (e) { out.push(e.name); }
globalThis.__result = JSON.stringify(out);
