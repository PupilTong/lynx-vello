// String comparison is by UTF-16 code units.
var out = ['a' < 'b', 'B' < 'a', 'abc' < 'abd', 'ab' < 'abc', '' < 'a', '\u00e9' > 'z', 'Z' < 'a', '\ud83d' < '\uffff'];
globalThis.__result = JSON.stringify(out);
