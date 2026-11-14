// Lookahead and lookbehind assertions, lazy quantifiers and word boundaries.
var out = [
  'price: $30, $45'.match(/(?<=\$)\d+/g), 'abc123'.match(/[a-z]+(?=\d)/)[0], 'abcd'.match(/b(?!c)/),
  'xay'.match(/(?<!x)a/), '<a><b>'.match(/<.+?>/)[0], 'cat concat'.match(/\bcat\b/g).length,
];
globalThis.__result = JSON.stringify(out);
