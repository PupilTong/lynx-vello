var name = 'world';
var greeting = `hello ${name}!`;
function tag(strings, ...values) { return strings.raw.join('|') + values.length; }
var tagged = tag`a${1}b\n${2}c`;
