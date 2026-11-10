// A property read site that sees several object shapes in a loop.
var shapes = [{ x: 1 }, { y: 2, x: 3 }, { z: 0, y: 1, x: 5 }, Object.create({ x: 7 }), { get x() { return 9; } }];
var sum = 0;
for (var i = 0; i < 50; i++) sum += shapes[i % shapes.length].x;
var keyed = 0;
for (var j = 0; j < 20; j++) keyed += [10, 20, 30][j % 3];
globalThis.__result = JSON.stringify([sum, keyed]);
