// super property references as destructuring targets, for-of and for-in targets
// and update operands in object literal methods; the stores land on this.
var proto = { n: 10, get viaGetter() { return 'proto getter ' + this.tag; } };
var obj = {
  __proto__: proto,
  tag: 'obj',
  named(src) {
    ({ a: super.a, b: super.b = 'default b' } = src);
    return [this.a, this.b, 'a' in proto];
  },
  computed(src, k1, k2) {
    ({ [k1]: super.c, [k2]: super.d } = src);
    return [this.c, this.d];
  },
  array(src) {
    [super.e, , super.f] = src;
    return [this.e, this.f];
  },
  update() {
    var before = super.n++;
    var pre = ++super.n;
    var post = super.n--;
    super.n += 100;
    return [before, pre, post, this.n, proto.n];
  },
  loop(list) {
    var seen = [];
    for (super.item of list) seen.push(this.item);
    for (super.key in { x: 1, y: 2 }) seen.push(this.key);
    return seen;
  },
  getter() { return super.viaGetter; },
};
globalThis.__result = JSON.stringify([
  obj.named({ a: 1 }),
  obj.computed({ p: 'P', q: 'Q' }, 'p', 'q'),
  obj.array([1, 2, 3]),
  obj.update(),
  obj.loop(['i', 'j']),
  obj.getter(),
]);
