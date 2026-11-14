function inspect(x) {
  var local = x * 2;
  return eval('local + x');
}
inspect(3);
