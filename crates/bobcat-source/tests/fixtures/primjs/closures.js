function counter(start) {
  let count = start;
  return {
    next: function () { count += 1; return count; },
    reset: () => { count = start; },
  };
}
var c = counter(10);
c.next();
c.reset();
