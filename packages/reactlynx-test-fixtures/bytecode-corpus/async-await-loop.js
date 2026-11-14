// await inside for, while and for-of loops.
async function f() {
  var out = [];
  for (let i = 0; i < 3; i++) out.push(await i);
  var n = 0;
  while (n < 2) n = await Promise.resolve(n + 1);
  out.push(n);
  for (const p of [Promise.resolve('a'), 'b']) out.push(await p);
  return out;
}
f().then(function (v) { globalThis.__result = JSON.stringify(v); });
