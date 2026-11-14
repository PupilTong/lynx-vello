// Assignment to an undeclared name in sloppy code creates a configurable global property.
function make() { implicitGlobal = 'made'; }
make();
var d = Object.getOwnPropertyDescriptor(globalThis, 'implicitGlobal');
implicitTop = 2;
implicitTop += 3;
globalThis.__result = JSON.stringify([implicitGlobal, d.configurable, d.enumerable, implicitTop]);
