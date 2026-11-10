// A function declared in a block in sloppy code is block scoped and also assigned to a var.
var out = [typeof blockFn];
{
  out.push(blockFn());
  function blockFn() { return 'inside'; }
}
out.push(typeof blockFn);
if (true) { function inIf() { return 'if'; } }
out.push(inIf());
globalThis.__result = JSON.stringify(out);
