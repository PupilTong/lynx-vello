// Globals created through globalThis are readable as identifiers.
globalThis.made = 'made';
var r1 = made;
globalThis.made = 'changed';
var r2 = made;
function f() { return made + ' in f'; }
delete globalThis.made;
var r3 = typeof made;
var r4;
try { made; } catch (e) { r4 = e.name; }
globalThis.__result = JSON.stringify([r1, r2, r3, r4, typeof f, globalThis === this]);
