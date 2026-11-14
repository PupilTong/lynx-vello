// Unary plus, minus, not, typeof and their coercions.
var out = [+'3', +'', +' 12 ', +'0x10', +[], +[5], +{}, -'4', !0, !!'', !!'0', +true, -null, +undefined, typeof -'a'];
globalThis.__result = JSON.stringify(out.map(String));
