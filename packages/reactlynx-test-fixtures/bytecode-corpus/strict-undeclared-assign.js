// Assignment to an undeclared name in strict code throws a ReferenceError.
function strictAssign() { 'use strict'; undeclaredStrictName = 1; }
var r;
try { strictAssign(); r = 'no error'; } catch (e) { r = e.name; }
var arrowStrict = function () { 'use strict'; return () => { alsoUndeclared += 1; }; };
var r2;
try { arrowStrict()(); } catch (e) { r2 = e.name; }
globalThis.__result = JSON.stringify([r, r2, typeof undeclaredStrictName]);
