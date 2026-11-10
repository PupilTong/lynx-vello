// Class declarations: constructor, methods, prototype shape, call without new.
class Point {
  constructor(x, y) { this.x = x; this.y = y; }
  sum() { return this.x + this.y; }
  toString() { return 'Point(' + this.x + ',' + this.y + ')'; }
}
var p = new Point(1, 2);
var err;
try { Point(); } catch (e) { err = e.name; }
var desc = Object.getOwnPropertyDescriptor(Point.prototype, 'sum');
globalThis.__result = JSON.stringify([p.sum(), String(p), typeof Point, err, desc.enumerable, Object.keys(p), p.constructor === Point]);
