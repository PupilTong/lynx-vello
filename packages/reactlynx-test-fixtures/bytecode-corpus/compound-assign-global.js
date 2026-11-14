// Compound assignment and ++/-- on global var, let and implicit globals.
var gv = 1;
let gl = 1;
gv += 1; gv++; ++gv; gv *= 3;
gl += 2; gl--; gl **= 2;
implicitG = 5;
implicitG -= 1;
implicitG++;
function fromFunction() { gv += 100; gl += 100; implicitG += 100; }
fromFunction();
var err;
try { missingGlobal += 1; } catch (e) { err = e.name; }
var err2;
try { missingGlobal2++; } catch (e) { err2 = e.name; }
globalThis.__result = JSON.stringify([gv, gl, implicitG, err, err2]);
