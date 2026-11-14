// Template literals: interpolation, nesting, multiple lines and escapes.
var a = 1, b = 'two';
var obj = { toString() { return 'OBJ'; } };
var s = `a=${a} b=${b} sum=${a + 2} obj=${obj} nested=${`inner ${a}`}`;
var multi = `line1
line2`;
var esc = `\${not} \` \u0041 \x42`;
globalThis.__result = JSON.stringify([s, multi, esc, `${null}${undefined}`, ``.length]);
