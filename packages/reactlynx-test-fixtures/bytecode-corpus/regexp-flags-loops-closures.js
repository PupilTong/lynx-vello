// RegExp literals with each flag PrimJS accepts (g, i, m, s, u, y) and combinations,
// evaluated inside loops and closures; each evaluation creates a new object.
var flags = [/a/g, /a/i, /^a/m, /a.b/s, /\u{61}/u, /a/y, /a/gimsuy];
var out = [flags.map(function (r) { return r.flags; })];
var counts = [];
for (var i = 0; i < 3; i++) {
  var re = /o/gy;
  var n = 0;
  while (re.test('ooxo')) n++;
  counts.push(n + '@' + re.lastIndex);
}
out.push(counts);
function makeMatcher(word) {
  return function (text) {
    var re = /(\w+)\s(\w+)/gimsu;
    var found = [];
    var m;
    while ((m = re.exec(text)) !== null) found.push(m[1] + '/' + m[2] + '/' + word);
    return found;
  };
}
var matcher = makeMatcher('w');
out.push(matcher('One two\nTHREE four'), matcher('x y'));
var objects = [];
var arrows = [() => /x/u, () => /x/u];
for (var j = 0; j < 2; j++) objects.push(arrows[j]());
out.push(objects[0] === objects[1], objects[0].source, /\u{1F600}/u.test('\u{1F600}'), /^.$/su.test('\n'));
out.push('a1b2c3'.replace(/\d/gy, '#'), 'ABC'.split(/(?:)/u), /[^]/s.exec('\n')[0] === '\n');
globalThis.__result = JSON.stringify(out);
