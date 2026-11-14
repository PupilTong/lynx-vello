// yield* in an async generator over async and sync iterables.
async function* inner() { yield 'i1'; await null; yield 'i2'; return 'inner ret'; }
async function* outer() { var r = yield* inner(); yield r; yield* ['s1', 's2']; }
async function main() {
  var out = [];
  for await (const v of outer()) out.push(v);
  return out;
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
