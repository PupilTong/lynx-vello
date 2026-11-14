// The sticky flag matches only at lastIndex.
var re = /\d+/y;
var out = [];
re.lastIndex = 0; out.push(re.test('12ab'), re.lastIndex);
re.lastIndex = 1; out.push(re.exec('12ab')[0], re.lastIndex);
re.lastIndex = 2; out.push(re.test('12ab'), re.lastIndex);
var tokens = [], tok = /\s*(\w+|[+*])/y, t;
while ((t = tok.exec('a + b * c')) !== null) tokens.push(t[1]);
out.push(tokens, re.sticky);
globalThis.__result = JSON.stringify(out);
