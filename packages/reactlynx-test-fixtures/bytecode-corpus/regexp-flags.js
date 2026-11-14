// RegExp literals with g, i, m and s flags.
var g = /a/g, i = /hello/i, m = /^b$/m, s = /a.b/s;
var out = ['xaxa'.replace(g, 'y'), i.test('HeLLo'), m.test('a\nb\nc'), s.test('a\nb'), /a.b/.test('a\nb'), g.flags, i.source, String(/x\/y/gi)];
globalThis.__result = JSON.stringify(out);
