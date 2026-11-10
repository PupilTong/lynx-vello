// The unicode flag: code point matching and \u{...} escapes.
var out = [
  /^.$/u.test('\ud83d\ude00'), /^.$/.test('\ud83d\ude00'), /\u{1F600}/u.test('\ud83d\ude00'),
  '\ud83d\ude00x'.match(/./gu).length, /[\u{1F600}-\u{1F64F}]/u.test('\ud83d\ude03'),
];
globalThis.__result = JSON.stringify(out);
