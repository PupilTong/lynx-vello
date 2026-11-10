// Abstract equality coercions.
var obj = { valueOf: function () { return 3; } };
var out = [null == undefined, null == 0, '' == 0, '1' == 1, true == 1, 'true' == true, NaN == NaN, obj == 3, [1] == 1, [] == '', [1, 2] == '1,2', null != undefined, 0 != false];
globalThis.__result = JSON.stringify(out);
