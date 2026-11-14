// super follows changes to the home object's prototype.
var p1 = { who() { return 'p1'; } };
var p2 = { who() { return 'p2'; } };
var o = { who() { return 'o>' + super.who(); } };
Object.setPrototypeOf(o, p1);
var r1 = o.who();
Object.setPrototypeOf(o, p2);
globalThis.__result = JSON.stringify([r1, o.who()]);
