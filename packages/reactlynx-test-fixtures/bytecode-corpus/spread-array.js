// Spread in array literals, including holes, strings and generators.
function* gen() { yield 'g1'; yield 'g2'; }
var holes = [1, , 3];
var out = [[...[1, 2], 3], [...'abc'], [...gen(), 'end'], [...holes], [...new Map([[1, 2]])], [0, ...[], 1]];
globalThis.__result = JSON.stringify([out, 1 in [...holes]]);
