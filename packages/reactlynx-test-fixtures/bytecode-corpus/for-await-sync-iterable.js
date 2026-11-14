// for await over a sync iterable awaits each value.
async function main() {
  var out = [];
  for await (const v of [Promise.resolve('a'), 'b', Promise.resolve('c')]) out.push(v);
  for await (const ch of 'xy') out.push(ch);
  try { for await (const v of [Promise.reject(new TypeError('t'))]) out.push(v); } catch (e) { out.push(e.name); }
  return out;
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
