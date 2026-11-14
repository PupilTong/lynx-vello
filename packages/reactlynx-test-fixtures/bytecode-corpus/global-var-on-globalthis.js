// Top-level var creates a non-deletable global property; let and const do not create one.
var gv = 1;
let gl = 2;
const gc = 3;
var d = Object.getOwnPropertyDescriptor(globalThis, 'gv');
globalThis.gv = 10;
var afterWrite = gv;
globalThis.__result = JSON.stringify(['gv' in globalThis, 'gl' in globalThis, 'gc' in globalThis, d.configurable, afterWrite, gl + gc]);
