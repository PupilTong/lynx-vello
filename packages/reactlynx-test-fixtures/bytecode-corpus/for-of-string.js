// for-of over a string iterates code points.
var out = [];
for (var ch of 'a\u00e9\ud83d\ude00z') out.push(ch.length);
var count = 0;
for (const c of '') count++;
globalThis.__result = JSON.stringify([out, count, [...'\ud83d\ude00'].length]);
