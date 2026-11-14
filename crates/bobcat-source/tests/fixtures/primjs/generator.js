function* numbers(limit) {
  for (let i = 0; i < limit; i++) yield i;
  yield* [limit, limit + 1];
}
var total = 0;
for (const n of numbers(3)) total += n;
