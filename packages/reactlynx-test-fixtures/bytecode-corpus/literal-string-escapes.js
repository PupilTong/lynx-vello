// String literal escapes and line continuations.
var s = 'a\tb\nc\\d\'e"f\x41\u0042\u{43}\0';
var t = "line one \
continued";
var u = '\u00e9\u4e2d\ud83d\ude00';
globalThis.__result = JSON.stringify([s, s.length, t, u, u.length, u.charCodeAt(2), '\v\f\b'.length]);
