// Ten nested functions; each level reads and writes variables from several levels
// up, a parameter of the outermost level and a global.
var g = 'g';
function f1(p1) {
  var v1 = 1;
  return function f2(p2) {
    let v2 = 2;
    return function f3(p3) {
      var v3 = 3;
      return function f4() {
        const v4 = 4;
        v1 += 10;
        return function f5() {
          var v5 = 5;
          return function f6() {
            let v6 = v1 + v2 + v3;
            return function f7() {
              v2 += 100;
              var v7 = v4 + v5;
              return function f8() {
                var v8 = [p1, p2, p3].join('');
                return function f9() {
                  v3 = v3 * 2;
                  return function f10() {
                    v5 += v6 + v7;
                    g += '!';
                    return [p1, p2, p3, v1, v2, v3, v4, v5, v6, v7, v8, g];
                  };
                };
              };
            };
          };
        };
      };
    };
  };
}
var f4 = f1('a')('b')('c');
var first = f4()()()()()()();
var second = f4()()()()()()();
globalThis.__result = JSON.stringify([first, second, g]);
