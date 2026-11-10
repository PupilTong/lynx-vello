async function load(values) {
  let sum = 0;
  for (const v of values) sum += await v;
  for await (const w of values) sum += w;
  return sum;
}
load([1, Promise.resolve(2)]).then(function (s) { globalThis.result = s; });
