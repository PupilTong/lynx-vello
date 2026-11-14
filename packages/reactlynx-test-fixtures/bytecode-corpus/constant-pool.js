// Many distinct literals and nested functions in one function's constant pool.
function f() {
  var nums = [1.5, 2.25, -3.125, 1e300, 5e-324, 2 ** 31, -(2 ** 31) - 1, 0.1];
  var strs = ['alpha', 'beta', 'gamma', '\u4e2d\u6587', '', 'a'.repeat(3)];
  var fns = [function () { return 1; }, () => 2, function* () { yield 3; }, async function () { return 4; }, class { m() { return 5; } }];
  return [nums.map(String), strs, fns[0]() + fns[1]() + fns[2]().next().value, typeof fns[3], new fns[4]().m()];
}
globalThis.__result = JSON.stringify(f());
