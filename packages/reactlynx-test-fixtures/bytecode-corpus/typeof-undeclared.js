// typeof on an undeclared name does not throw; reading it does.
var r1 = typeof neverDeclaredAnywhere;
var r2;
try { neverDeclaredAnywhere; r2 = 'no error'; } catch (e) { r2 = e.name; }
function inner() { return typeof alsoNeverDeclared; }
var r3 = inner();
var r4 = typeof globalThis.neverDeclaredAnywhere;
globalThis.__result = JSON.stringify([r1, r2, r3, r4]);
