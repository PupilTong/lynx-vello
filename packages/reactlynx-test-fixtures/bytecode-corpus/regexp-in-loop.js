// A regexp literal in a loop is a new object per evaluation; lastIndex does not carry over.
var objects = [], results = [];
for (var i = 0; i < 3; i++) {
  var re = /o/g;
  objects.push(re);
  results.push(re.test('foo'), re.lastIndex);
}
var shared = /o/g, sharedResults = [];
for (var j = 0; j < 4; j++) sharedResults.push(shared.test('foo'));
globalThis.__result = JSON.stringify([objects[0] === objects[1], results, sharedResults]);
