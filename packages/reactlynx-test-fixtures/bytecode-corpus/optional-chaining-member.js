// Optional member access on null, undefined and objects.
var o = { a: { b: { c: 1 } }, arr: [10, 20], n: null };
var u;
var key = 'b';
var out = [o?.a?.b?.c, o.n?.x, u?.x, u?.[key], o.a?.[key].c, o.arr?.[1], o.missing?.deep.deeper, (o?.a).b.c];
globalThis.__result = JSON.stringify(out.map(String));
