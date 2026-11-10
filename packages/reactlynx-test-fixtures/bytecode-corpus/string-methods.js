// Common String.prototype methods.
var s = '  Hello, World  ';
var t = s.trim();
var out = [t.toUpperCase(), t.toLowerCase(), t.indexOf('o'), t.lastIndexOf('o'), t.slice(-5), t.substring(7, 2), t.split(', '), t.padStart(15, '*'), t.padEnd(14, '-'), 'ab'.repeat(3), t.startsWith('Hell'), t.endsWith('!'), t.charAt(1), t.codePointAt(0), s.trimStart().length, 'a-b-c'.split('-', 2), t.at(-1), 'abc'.localeCompare('abd') < 0];
globalThis.__result = JSON.stringify(out);
