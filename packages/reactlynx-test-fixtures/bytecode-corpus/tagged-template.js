// Tagged templates receive cooked strings, raw strings and substitutions.
function tag(strings, ...subs) { return { cooked: Array.from(strings), raw: strings.raw, subs: subs, frozen: Object.isFrozen(strings) }; }
var x = 5;
var r = tag`a${x}b${x * 2}\n${'s'}`;
var bad = tag`\unicode`;
var o = { m(strings) { return this === o; } };
globalThis.__result = JSON.stringify([r, bad.cooked[0] === undefined, bad.raw[0], o.m`x`, String.raw`a\n${1}`]);
